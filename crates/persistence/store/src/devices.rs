//! Paired devices and their pairing codes
//! (docs/harness/remote-computer-use.md §4).
//!
//! Secrets are never stored: a device's token and a pairing code are kept as
//! hashes the caller computed, so this module only ever compares hashes.
//! Every read that serves a tenant filters on it.

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, Row, params};

use crate::SQLiteStore;
use crate::crud::{now_rfc3339, parse_rfc3339};

#[derive(Debug, Clone, PartialEq)]
pub struct DeviceRecord {
    pub device_id: String,
    pub tenant_id: String,
    /// Who paired it.
    pub principal_id: String,
    pub name: String,
    pub platform: String,
    pub token_hash: String,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DevicePairingRecord {
    pub code_hash: String,
    pub tenant_id: String,
    pub principal_id: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

const DEVICE_COLUMNS: &str = "device_id, tenant_id, principal_id, name, platform, token_hash, \
     created_at, last_seen_at, revoked_at";

fn device_from_row(row: &Row<'_>) -> rusqlite::Result<[Option<String>; 9]> {
    let mut columns: [Option<String>; 9] = Default::default();
    for (index, column) in columns.iter_mut().enumerate() {
        *column = row.get(index)?;
    }
    Ok(columns)
}

fn device_from_columns(columns: [Option<String>; 9]) -> Result<DeviceRecord, String> {
    let [
        device_id,
        tenant_id,
        principal_id,
        name,
        platform,
        token_hash,
        created_at,
        last_seen_at,
        revoked_at,
    ] = columns;
    let optional_time = |value: Option<String>| value.as_deref().map(parse_rfc3339).transpose();
    Ok(DeviceRecord {
        device_id: device_id.unwrap_or_default(),
        tenant_id: tenant_id.unwrap_or_default(),
        principal_id: principal_id.unwrap_or_default(),
        name: name.unwrap_or_default(),
        platform: platform.unwrap_or_default(),
        token_hash: token_hash.unwrap_or_default(),
        created_at: parse_rfc3339(created_at.as_deref().unwrap_or_default())?,
        last_seen_at: optional_time(last_seen_at)?,
        revoked_at: optional_time(revoked_at)?,
    })
}

impl SQLiteStore {
    pub fn insert_device_pairing(&self, pairing: &DevicePairingRecord) -> Result<(), String> {
        self.conn
            .execute(
                r#"INSERT INTO device_pairings
                    (code_hash, tenant_id, principal_id, created_at, expires_at)
                VALUES (?1, ?2, ?3, ?4, ?5)"#,
                params![
                    pairing.code_hash,
                    pairing.tenant_id,
                    pairing.principal_id,
                    now_rfc3339(&pairing.created_at),
                    now_rfc3339(&pairing.expires_at),
                ],
            )
            .map_err(|e| format!("insert device pairing: {e}"))?;
        Ok(())
    }

    /// Spends the pairing code and creates the device it pays for, in one
    /// transaction: a code works once, and only before it expires. The
    /// device takes the code's tenant and principal; the caller's are
    /// ignored. `None` when the code is unknown, spent or expired, which the
    /// caller must not tell apart for the client.
    pub fn redeem_device_pairing(
        &self,
        code_hash: &str,
        now: &DateTime<Utc>,
        device: &DeviceRecord,
    ) -> Result<Option<DeviceRecord>, String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("begin redeem device pairing: {e}"))?;
        let owner = tx
            .query_row(
                r#"SELECT tenant_id, principal_id FROM device_pairings
                WHERE code_hash = ?1 AND used_at IS NULL AND expires_at > ?2"#,
                params![code_hash, now_rfc3339(now)],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|e| format!("read device pairing: {e}"))?;
        let Some((tenant_id, principal_id)) = owner else {
            return Ok(None);
        };
        tx.execute(
            "UPDATE device_pairings SET used_at = ?2 WHERE code_hash = ?1",
            params![code_hash, now_rfc3339(now)],
        )
        .map_err(|e| format!("spend device pairing: {e}"))?;
        let device = DeviceRecord {
            tenant_id,
            principal_id,
            created_at: *now,
            last_seen_at: None,
            revoked_at: None,
            ..device.clone()
        };
        tx.execute(
            &format!(
                "INSERT INTO devices ({DEVICE_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL)"
            ),
            params![
                device.device_id,
                device.tenant_id,
                device.principal_id,
                device.name,
                device.platform,
                device.token_hash,
                now_rfc3339(&device.created_at),
            ],
        )
        .map_err(|e| format!("insert device: {e}"))?;
        tx.commit()
            .map_err(|e| format!("commit redeem device pairing: {e}"))?;
        Ok(Some(device))
    }

    /// The device holding this token hash, revoked or not; the caller
    /// decides what a revoked device may do (nothing).
    pub fn device_by_token_hash(&self, token_hash: &str) -> Result<Option<DeviceRecord>, String> {
        let columns = self
            .conn
            .query_row(
                &format!("SELECT {DEVICE_COLUMNS} FROM devices WHERE token_hash = ?1"),
                params![token_hash],
                device_from_row,
            )
            .optional()
            .map_err(|e| format!("read device by token: {e}"))?;
        columns.map(device_from_columns).transpose()
    }

    pub fn get_device(
        &self,
        tenant_id: &str,
        device_id: &str,
    ) -> Result<Option<DeviceRecord>, String> {
        let columns = self
            .conn
            .query_row(
                &format!(
                    "SELECT {DEVICE_COLUMNS} FROM devices WHERE tenant_id = ?1 AND device_id = ?2"
                ),
                params![tenant_id, device_id],
                device_from_row,
            )
            .optional()
            .map_err(|e| format!("read device {device_id}: {e}"))?;
        columns.map(device_from_columns).transpose()
    }

    /// A tenant's devices, oldest first, revoked ones included.
    pub fn list_devices(&self, tenant_id: &str) -> Result<Vec<DeviceRecord>, String> {
        let mut statement = self
            .conn
            .prepare(&format!(
                "SELECT {DEVICE_COLUMNS} FROM devices WHERE tenant_id = ?1 \
                 ORDER BY created_at ASC, device_id ASC"
            ))
            .map_err(|e| format!("list devices: {e}"))?;
        let rows = statement
            .query_map(params![tenant_id], device_from_row)
            .map_err(|e| format!("list devices: {e}"))?;
        let mut devices = Vec::new();
        for row in rows {
            devices.push(device_from_columns(
                row.map_err(|e| format!("list devices: {e}"))?,
            )?);
        }
        Ok(devices)
    }

    /// Revokes a tenant's device. `false` when the tenant has no such device
    /// or it was already revoked; the first revocation time is kept.
    pub fn revoke_device(
        &self,
        tenant_id: &str,
        device_id: &str,
        now: &DateTime<Utc>,
    ) -> Result<bool, String> {
        let changed = self
            .conn
            .execute(
                r#"UPDATE devices SET revoked_at = ?3
                WHERE tenant_id = ?1 AND device_id = ?2 AND revoked_at IS NULL"#,
                params![tenant_id, device_id, now_rfc3339(now)],
            )
            .map_err(|e| format!("revoke device {device_id}: {e}"))?;
        Ok(changed == 1)
    }

    /// Records that a device connected or left; once per connection, never
    /// per frame.
    pub fn touch_device(&self, device_id: &str, now: &DateTime<Utc>) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE devices SET last_seen_at = ?2 WHERE device_id = ?1",
                params![device_id, now_rfc3339(now)],
            )
            .map_err(|e| format!("touch device {device_id}: {e}"))?;
        Ok(())
    }

    /// Deletes pairing codes that expired before `cutoff`, spent or not;
    /// returns how many.
    pub fn prune_device_pairings(&self, cutoff: &DateTime<Utc>) -> Result<usize, String> {
        self.conn
            .execute(
                "DELETE FROM device_pairings WHERE expires_at < ?1",
                params![now_rfc3339(cutoff)],
            )
            .map_err(|e| format!("prune device pairings: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn temp_dir(name: &str) -> String {
        let dir =
            std::env::temp_dir().join(format!("kura_store_devices_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.to_string_lossy().to_string()
    }

    fn pairing(code_hash: &str, tenant: &str, expires_at: DateTime<Utc>) -> DevicePairingRecord {
        DevicePairingRecord {
            code_hash: code_hash.into(),
            tenant_id: tenant.into(),
            principal_id: format!("prn_{tenant}"),
            created_at: Utc::now(),
            expires_at,
        }
    }

    fn device(device_id: &str, token_hash: &str) -> DeviceRecord {
        DeviceRecord {
            device_id: device_id.into(),
            // Ignored by redeem: the code decides whose device it is.
            tenant_id: "forged".into(),
            principal_id: "forged".into(),
            name: "work mac".into(),
            platform: "macos".into(),
            token_hash: token_hash.into(),
            created_at: Utc::now(),
            last_seen_at: None,
            revoked_at: None,
        }
    }

    #[test]
    fn a_code_pairs_one_device_once_and_for_its_own_tenant() {
        let store = SQLiteStore::new(&temp_dir("redeem")).unwrap();
        let now = Utc::now();
        store
            .insert_device_pairing(&pairing("c1", "ten_a", now + Duration::minutes(10)))
            .unwrap();

        let paired = store
            .redeem_device_pairing("c1", &now, &device("dev_1", "t1"))
            .unwrap()
            .unwrap();
        assert_eq!(paired.tenant_id, "ten_a");
        assert_eq!(paired.principal_id, "prn_ten_a");
        // Spent.
        assert!(
            store
                .redeem_device_pairing("c1", &now, &device("dev_2", "t2"))
                .unwrap()
                .is_none()
        );
        // Unknown.
        assert!(
            store
                .redeem_device_pairing("nope", &now, &device("dev_3", "t3"))
                .unwrap()
                .is_none()
        );

        let found = store.device_by_token_hash("t1").unwrap().unwrap();
        assert_eq!(found, paired);
        assert!(store.device_by_token_hash("t2").unwrap().is_none());
    }

    #[test]
    fn an_expired_code_pairs_nothing_and_is_pruned() {
        let store = SQLiteStore::new(&temp_dir("expired")).unwrap();
        let now = Utc::now();
        store
            .insert_device_pairing(&pairing("old", "ten_a", now - Duration::seconds(1)))
            .unwrap();
        assert!(
            store
                .redeem_device_pairing("old", &now, &device("dev_1", "t1"))
                .unwrap()
                .is_none()
        );
        assert!(store.device_by_token_hash("t1").unwrap().is_none());
        assert_eq!(store.prune_device_pairings(&now).unwrap(), 1);
    }

    #[test]
    fn devices_are_listed_and_revoked_within_their_tenant() {
        let store = SQLiteStore::new(&temp_dir("tenant")).unwrap();
        let now = Utc::now();
        for (code, tenant, id, token) in [
            ("ca", "ten_a", "dev_a", "ta"),
            ("cb", "ten_b", "dev_b", "tb"),
        ] {
            store
                .insert_device_pairing(&pairing(code, tenant, now + Duration::minutes(10)))
                .unwrap();
            store
                .redeem_device_pairing(code, &now, &device(id, token))
                .unwrap()
                .unwrap();
        }

        let listed = store.list_devices("ten_a").unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].device_id, "dev_a");
        assert!(store.get_device("ten_a", "dev_b").unwrap().is_none());

        // Another tenant's device cannot be revoked.
        assert!(!store.revoke_device("ten_a", "dev_b", &now).unwrap());
        assert!(store.revoke_device("ten_a", "dev_a", &now).unwrap());
        assert!(!store.revoke_device("ten_a", "dev_a", &now).unwrap());
        let revoked = store.get_device("ten_a", "dev_a").unwrap().unwrap();
        assert_eq!(revoked.revoked_at.unwrap().timestamp(), now.timestamp());
        assert!(
            store
                .get_device("ten_b", "dev_b")
                .unwrap()
                .unwrap()
                .revoked_at
                .is_none()
        );

        store.touch_device("dev_b", &now).unwrap();
        assert!(
            store
                .get_device("ten_b", "dev_b")
                .unwrap()
                .unwrap()
                .last_seen_at
                .is_some()
        );
    }

    #[test]
    fn a_v8_database_gains_the_tables_on_upgrade() {
        let dir = temp_dir("upgrade");
        {
            let store = SQLiteStore::new_at_version(&dir, 8).unwrap();
            assert!(store.list_devices("ten_a").is_err());
        }
        let store = SQLiteStore::new(&dir).unwrap();
        assert_eq!(
            store.schema_version().unwrap(),
            crate::CURRENT_SCHEMA_VERSION
        );
        assert!(store.list_devices("ten_a").unwrap().is_empty());
    }
}
