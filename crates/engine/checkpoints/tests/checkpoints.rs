use std::sync::Arc;

use kura_checkpoints::Manager;
use kura_runtime::{
    CreateRunInput, CreateStepInput, CreateToolCallInput, Manager as RuntimeManager,
};
use kura_store::SQLiteStore;

fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("kura_checkpoints_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

#[test]
fn saves_and_restores_latest_checkpoint() {
    let dir = temp_dir("restore");
    let store = Arc::new(parking_lot::Mutex::new(SQLiteStore::new(&dir).unwrap()));
    let runtime = Arc::new(RuntimeManager::new());
    let manager = Manager::new(store.clone(), runtime.clone());

    let run = runtime
        .create_run(CreateRunInput {
            run_id: "run_cp".to_string(),
            entrypoint: "test entrypoint".to_string(),
            goal: "test goal".to_string(),
            ..CreateRunInput::default()
        })
        .unwrap();
    assert_eq!(run.run_id, "run_cp");

    let step = runtime
        .create_step(
            "run_cp",
            CreateStepInput {
                title: "Do work".to_string(),
                kind: "task".to_string(),
                ..CreateStepInput::default()
            },
        )
        .unwrap();

    runtime
        .create_tool_call(
            "run_cp",
            &step.step_id,
            CreateToolCallInput {
                tool_name: "search".to_string(),
                capability_id: "cap_1".to_string(),
                ..CreateToolCallInput::default()
            },
        )
        .unwrap();

    manager.save_run_checkpoint("run_cp").unwrap();

    // The checkpoint row was persisted.
    assert_eq!(store.lock().list_latest_checkpoints().unwrap().len(), 1);

    // A fresh runtime recovers the run from the latest checkpoint.
    let runtime2 = Arc::new(RuntimeManager::new());
    let manager2 = Manager::new(store.clone(), runtime2.clone());
    let stats = manager2.restore().unwrap();
    assert_eq!(stats.run_count, 1);

    let restored = runtime2.get_run("run_cp").expect("run restored");
    assert_eq!(restored.goal, "test goal");
    let steps = runtime2.list_steps("run_cp").unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].title, "Do work");
    let tool_calls = runtime2
        .list_tool_calls("run_cp", &steps[0].step_id)
        .unwrap();
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].tool_name, "search");
}
/// Compile-time guard: this manager must be usable from axum `AppState` (Send + Sync).
#[test]
fn manager_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<kura_checkpoints::Manager>();
}
