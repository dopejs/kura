//! Running rounds (work-items.md §5.3).
//!
//! A round is a non-streamed chat turn: request id `<item>_r<seq>`, the
//! item's own thread, the mandate at the head of its system frame, and the
//! report schema as its response format. [`Manager::drive`] runs one round
//! after another for as long as the item has pending inputs and nothing
//! holds it, one round at a time per item; turn admission is the only
//! limiter across items.
//!
//! Slice 2 has no daemon-wide driver task. Rounds are driven by whoever
//! woke the item: the API route and `work_item.open` on a thread of their
//! own, the IM hook inline on the connector loop, as an ordinary IM turn
//! runs today (its reply goes through that loop's reply path until the
//! outbox, slice 3). A wake that finds a round running leaves its input to
//! that round's driver, which looks again after it settles.

use std::sync::Arc;

use kura_chat::{Admitted, CancellationToken, ChatError, QueryInput};

use crate::manager::{Manager, RoundOutcome, RoundTurn};
use crate::types::Report;
use crate::wake::NoRound;

/// How one round the driver ran ended.
#[derive(Debug, Clone, PartialEq)]
pub struct DrivenRound {
    pub seq: i64,
    /// The report as recorded (a reply that was not one is recorded as a
    /// `blocked` report with reason `report_unreadable`).
    pub report: Option<Report>,
    /// The chat error code of a turn that ended without a report.
    pub error_code: Option<String>,
    /// The approval the item now waits on, when a tool call of this round
    /// gave up waiting on it (slice 5).
    pub waits_on: Option<crate::types::Attention>,
}

/// Why a drive stopped before running a round it might have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriveStop {
    /// Turn admission refused the round; its inputs stay pending.
    Throttled { code: String },
    /// The item started its rounds for this hour; its inputs stay pending.
    HourlyCap { limit: u32 },
}

/// What a drive did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Drive {
    pub rounds: Vec<DrivenRound>,
    pub stopped: Option<DriveStop>,
}

impl Manager {
    /// Runs `first` (already started and admitted), then every round the
    /// item has due, one at a time, until nothing is due. Blocking.
    pub fn drive(
        &self,
        tenant_id: &str,
        item_id: &str,
        first: Option<(RoundTurn, Admitted)>,
    ) -> Drive {
        let mut drive = Drive::default();
        if let Some((turn, admitted)) = first {
            drive.rounds.push(self.run_round(&turn, admitted));
        }
        loop {
            match self.round_due(tenant_id, item_id) {
                Ok(Ok(())) => {}
                Ok(Err(NoRound::HourlyCap { limit })) => {
                    drive.stopped = Some(DriveStop::HourlyCap { limit });
                    break;
                }
                Ok(Err(_)) => break,
                Err(err) => {
                    log_drive_failure(tenant_id, item_id, &err.to_string());
                    break;
                }
            }
            // Admitted before the round is recorded: a refusal records
            // nothing and the inputs stay pending for the next wake.
            let admitted = match self.admit(tenant_id) {
                Ok(admitted) => admitted,
                Err(err) => {
                    drive.stopped = Some(DriveStop::Throttled {
                        code: err.code().to_string(),
                    });
                    break;
                }
            };
            let turn = match self.start_round(tenant_id, item_id) {
                Ok(Ok(turn)) => turn,
                // Another wake started it, or the item moved on: the slot
                // is released with `admitted`.
                Ok(Err(NoRound::HourlyCap { limit })) => {
                    drive.stopped = Some(DriveStop::HourlyCap { limit });
                    break;
                }
                Ok(Err(_)) => break,
                Err(err) => {
                    log_drive_failure(tenant_id, item_id, &err.to_string());
                    break;
                }
            };
            drive.rounds.push(self.run_round(&turn, admitted));
        }
        drive
    }

    /// Runs the first round of an item just opened by someone who could not
    /// wait for it (`work_item.open` inside a turn, which already holds a
    /// slot): on a thread of its own, admitted there, then drives the item
    /// on. A refused admission is recorded as a failed round.
    pub fn spawn_first_round(self: &Arc<Self>, turn: RoundTurn) {
        let manager = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("kura-work-item-round".to_string())
            .spawn(move || {
                let admitted = match manager.admit(&turn.tenant_id) {
                    Ok(admitted) => admitted,
                    Err(err) => {
                        manager.settle_quietly(
                            &turn,
                            RoundOutcome::Failed {
                                error_code: err.code().to_string(),
                                turn_id: None,
                            },
                        );
                        return;
                    }
                };
                let (tenant_id, item_id) = (turn.tenant_id.clone(), turn.item_id.clone());
                manager.drive(&tenant_id, &item_id, Some((turn, admitted)));
            });
        if let Err(err) = spawned {
            eprintln!("[kura] work item: spawning a round thread failed: {err}");
        }
    }

    /// [`Self::drive`] on a thread of its own, for a first round its caller
    /// already admitted (`POST /v1/work-items`).
    pub fn spawn_drive(
        self: &Arc<Self>,
        turn: RoundTurn,
        admitted: Admitted,
    ) -> std::io::Result<()> {
        let manager = Arc::clone(self);
        std::thread::Builder::new()
            .name("kura-work-item-round".to_string())
            .spawn(move || {
                let (tenant_id, item_id) = (turn.tenant_id.clone(), turn.item_id.clone());
                manager.drive(&tenant_id, &item_id, Some((turn, admitted)));
            })
            .map(|_| ())
    }

    fn admit(&self, tenant_id: &str) -> Result<Admitted, ChatError> {
        match &self.chat {
            Some(chat) => chat.admit(tenant_id),
            None => Err(ChatError::NotConfigured),
        }
    }

    /// Runs one round as a chat turn holding `admitted`, then records how
    /// it ended. Blocking. A panic in the turn is recorded as a failed round
    /// rather than leaving it running.
    pub fn run_round(&self, turn: &RoundTurn, admitted: Admitted) -> DrivenRound {
        let outcome = match &self.chat {
            Some(chat) => run_turn(chat, turn, admitted),
            None => RoundOutcome::Failed {
                error_code: "chat_unavailable".to_string(),
                turn_id: None,
            },
        };
        let failed = match &outcome {
            RoundOutcome::Failed { error_code, .. } => Some(error_code.clone()),
            _ => None,
        };
        match self.settle_round(turn, outcome) {
            Ok(item) => DrivenRound {
                seq: turn.seq,
                // A failed round leaves the last report as it was: it is
                // not this round's.
                report: if failed.is_some() {
                    None
                } else {
                    item.last_report
                },
                waits_on: item
                    .attention
                    .filter(|attention| attention.kind == crate::types::AttentionKind::Approval),
                error_code: failed,
            },
            Err(err) => {
                log_drive_failure(
                    &turn.tenant_id,
                    &turn.item_id,
                    &format!("settling round {} failed: {err}", turn.seq),
                );
                DrivenRound {
                    seq: turn.seq,
                    report: None,
                    waits_on: None,
                    error_code: Some(failed.unwrap_or_else(|| err.code().to_string())),
                }
            }
        }
    }

    fn settle_quietly(&self, turn: &RoundTurn, outcome: RoundOutcome) {
        if let Err(err) = self.settle_round(turn, outcome) {
            log_drive_failure(
                &turn.tenant_id,
                &turn.item_id,
                &format!("settling round {} failed: {err}", turn.seq),
            );
        }
    }
}

/// The round's turn, and how it ended.
fn run_turn(chat: &kura_chat::Service, turn: &RoundTurn, admitted: Admitted) -> RoundOutcome {
    let input = QueryInput {
        query: turn.query_text.clone(),
        provider: turn.provider.clone(),
        model: turn.model.clone(),
        tenant_id: turn.tenant_id.clone(),
        thread_id: turn.thread_id.clone(),
        request_id: turn.request_id.clone(),
        instructions: turn.instructions.clone(),
        response_format: Some(kura_llm::ResponseFormat {
            name: crate::report::REPORT_FORMAT_NAME.to_string(),
            schema: crate::report::report_schema(),
        }),
        ..QueryInput::default()
    };
    let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        chat.query_admitted(admitted, input, &CancellationToken::new())
    }));
    match ran {
        Ok(Ok(execution)) => {
            let turn_id = Some(execution.result.turn_id.clone()).filter(|id| !id.is_empty());
            match execution.exec_error {
                None => RoundOutcome::Answered {
                    structured: execution.result.structured_output,
                    turn_id,
                },
                Some(err) => {
                    log_turn_failure(turn, &err.to_string());
                    RoundOutcome::Failed {
                        error_code: Some(execution.result.dispatch.error_code.clone())
                            .filter(|code| !code.is_empty())
                            .unwrap_or_else(|| err.code().to_string()),
                        turn_id,
                    }
                }
            }
        }
        Ok(Err(ChatError::StructuredOutputInvalid(detail))) => RoundOutcome::Unreadable {
            detail,
            turn_id: None,
        },
        Ok(Err(err)) => {
            log_turn_failure(turn, &err.to_string());
            RoundOutcome::Failed {
                error_code: err.code().to_string(),
                turn_id: None,
            }
        }
        Err(_) => {
            log_turn_failure(turn, "the turn panicked");
            RoundOutcome::Failed {
                error_code: "panic".to_string(),
                turn_id: None,
            }
        }
    }
}

/// The round's record keeps only the error code; the message goes to the
/// log, where an operator looks for why.
fn log_turn_failure(turn: &RoundTurn, message: &str) {
    eprintln!(
        "[kura] work item: tenant={} item={} round {} ended without a report: {message}",
        turn.tenant_id, turn.item_id, turn.seq
    );
}

fn log_drive_failure(tenant_id: &str, item_id: &str, message: &str) {
    eprintln!("[kura] work item: tenant={tenant_id} item={item_id} driver: {message}");
}
