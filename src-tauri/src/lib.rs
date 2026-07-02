use stdf_core::parser::{ParseErrorEvent, ParseProgress};
use stdf_core::sessions::{
    EnrichedField, RecordGroup, RecordSummaryPage, SearchProgress, SearchResultPage,
    SessionManager, SessionSnapshot, TestItemColumnLite, TestItemPage, TestItemViewSnapshot,
};
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, State};

#[tauri::command]
fn open_stdf(
    path: String,
    app: AppHandle,
    manager: State<'_, SessionManager>,
) -> Result<stdf_core::sessions::ParseSession, String> {
    manager.open_stdf(path, move |event| match event {
        stdf_core::sessions::SessionEvent::Progress(progress) => {
            let _ = app.emit("parse-progress", progress);
        }
        stdf_core::sessions::SessionEvent::Snapshot(snapshot) => {
            let _ = app.emit("session-snapshot", snapshot);
        }
        stdf_core::sessions::SessionEvent::RecordBatch(batch) => {
            let _ = app.emit("record-batch", batch);
        }
        stdf_core::sessions::SessionEvent::Complete(session_id) => {
            let _ = app.emit("parse-complete", session_id);
        }
        stdf_core::sessions::SessionEvent::Warning(warning) => {
            let _ = app.emit("parse-warning", warning);
        }
        stdf_core::sessions::SessionEvent::Error(error) => {
            let _ = app.emit("parse-error", error);
        }
    })
}

#[tauri::command]
fn cancel_parse(session_id: String, manager: State<'_, SessionManager>) -> Result<(), String> {
    manager.cancel_parse(&session_id)
}

#[tauri::command]
fn get_session_snapshot(
    session_id: String,
    manager: State<'_, SessionManager>,
) -> Result<SessionSnapshot, String> {
    manager.get_session_snapshot(&session_id)
}

#[tauri::command]
fn get_test_item_view(
    session_id: String,
    manager: State<'_, SessionManager>,
) -> Result<TestItemViewSnapshot, String> {
    manager.get_test_item_view(&session_id)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn get_test_item_page(
    session_id: String,
    row_offset: usize,
    row_count: usize,
    col_offset: usize,
    col_count: usize,
    selected: Vec<String>,
    site_filter: String,
    manager: State<'_, SessionManager>,
) -> Result<TestItemPage, String> {
    manager.get_test_item_page(
        &session_id,
        row_offset,
        row_count,
        col_offset,
        col_count,
        &selected,
        &site_filter,
    )
}

#[tauri::command]
fn get_test_item_columns(
    session_id: String,
    manager: State<'_, SessionManager>,
) -> Result<Vec<TestItemColumnLite>, String> {
    manager.get_test_item_columns(&session_id)
}

#[tauri::command]
fn export_test_item_csv(
    session_id: String,
    path: String,
    manager: State<'_, SessionManager>,
) -> Result<(), String> {
    manager.export_test_item_csv(&session_id, &path)
}

#[tauri::command]
fn get_record_groups(
    session_id: String,
    manager: State<'_, SessionManager>,
) -> Result<Vec<RecordGroup>, String> {
    manager.get_record_groups(&session_id)
}

#[tauri::command]
fn get_records(
    session_id: String,
    group: String,
    page: usize,
    page_size: usize,
    manager: State<'_, SessionManager>,
) -> Result<RecordSummaryPage, String> {
    manager.get_records(&session_id, &group, page, page_size)
}

#[tauri::command]
fn get_record_fields(
    session_id: String,
    record_id: String,
    manager: State<'_, SessionManager>,
) -> Result<Vec<EnrichedField>, String> {
    manager.get_record_fields(&session_id, &record_id)
}

// `(async)` runs this sync CPU-bound handler on a spawned tokio task instead
// of the WebKit main thread — the search loop otherwise blocks the UI for the
// full 30-60s scan, freezing the cursor and starving progress callbacks.
#[tauri::command(async)]
fn search_fields(
    session_id: String,
    query: String,
    page: usize,
    page_size: usize,
    on_progress: Channel<SearchProgress>,
    manager: State<'_, SessionManager>,
) -> Result<SearchResultPage, String> {
    // Report progress via a Tauri v2 Channel scoped to this invoke rather than
    // a global emit/listen event bus — the channel is bound to the caller's
    // Promise, so ordering is guaranteed and there is no window where the JS
    // side "hasn't subscribed yet".
    let sid = session_id.clone();
    manager.search_fields(&session_id, &query, page, page_size, move |scanned, total| {
        let _ = on_progress.send(SearchProgress {
            session_id: sid.clone(),
            scanned,
            total,
        });
    })
}

pub fn run() {
    // Clean any decompressed temp files left over from a previous run/crash.
    let _ = std::fs::remove_dir_all(stdf_core::sessions::temp_workspace_dir());
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(SessionManager::default())
        .invoke_handler(tauri::generate_handler![
            open_stdf,
            cancel_parse,
            get_session_snapshot,
            get_test_item_view,
            get_test_item_page,
            get_test_item_columns,
            export_test_item_csv,
            get_record_groups,
            get_records,
            get_record_fields,
            search_fields
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[allow(dead_code)]
fn _event_types(_: ParseProgress, _: ParseErrorEvent) {}
