use stdf_core::sessions::{
    BinSummary, DtrParseResult, EnrichedField, GdrParseResult, MprPinDetails, RecordGroup,
    RecordSummaryPage, SearchProgress, SearchResultPage, SessionManager, SessionSnapshot,
    TestItemColumnLite, TestItemPage,
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

// `(async)` on the test-item commands: they materialize a page of the part
// matrix (up to 500 rows × 1000 cols), which is enough work that dispatching
// on a tokio task keeps the WebKit main thread and the mouse responsive.
#[tauri::command(async)]
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

#[tauri::command(async)]
fn get_test_item_columns(
    session_id: String,
    manager: State<'_, SessionManager>,
) -> Result<Vec<TestItemColumnLite>, String> {
    manager.get_test_item_columns(&session_id)
}

#[tauri::command(async)]
fn get_bin_summary(
    session_id: String,
    manager: State<'_, SessionManager>,
) -> Result<BinSummary, String> {
    manager.get_bin_summary(&session_id)
}

#[tauri::command(async)]
fn export_test_item_csv(
    session_id: String,
    path: String,
    manager: State<'_, SessionManager>,
) -> Result<(), String> {
    manager.export_test_item_csv(&session_id, &path)
}

// `(async)`: both re-read the source file / copy a potentially large txt, so
// keep them off the WebKit main thread like the other IO-heavy commands.
#[tauri::command(async)]
fn parse_dtr_text(
    session_id: String,
    manager: State<'_, SessionManager>,
) -> Result<DtrParseResult, String> {
    manager.parse_dtr_text(&session_id)
}

#[tauri::command(async)]
fn save_dtr_text(
    session_id: String,
    path: String,
    manager: State<'_, SessionManager>,
) -> Result<(), String> {
    manager.save_dtr_text(&session_id, &path)
}

#[tauri::command(async)]
fn parse_gdr_text(
    session_id: String,
    manager: State<'_, SessionManager>,
) -> Result<GdrParseResult, String> {
    manager.parse_gdr_text(&session_id)
}

#[tauri::command(async)]
fn save_gdr_text(
    session_id: String,
    path: String,
    manager: State<'_, SessionManager>,
) -> Result<(), String> {
    manager.save_gdr_text(&session_id, &path)
}

// `(async)`: expanding an MPR cell re-reads (and possibly re-decompresses)
// the source file up to the clicked record — IO-heavy like parse_dtr_text.
#[tauri::command(async)]
fn get_mpr_pin_details(
    session_id: String,
    test_num: u32,
    record_position: usize,
    manager: State<'_, SessionManager>,
) -> Result<MprPinDetails, String> {
    manager.get_mpr_pin_details(&session_id, test_num, record_position)
}

// `(async)`: same on-demand source re-read as get_mpr_pin_details, plus the
// xlsx write itself.
#[tauri::command(async)]
#[allow(clippy::too_many_arguments)]
fn export_mpr_pins_xlsx(
    session_id: String,
    test_num: u32,
    record_position: usize,
    part_id: String,
    site_num: String,
    path: String,
    manager: State<'_, SessionManager>,
) -> Result<(), String> {
    manager.export_mpr_pins_xlsx(
        &session_id,
        test_num,
        record_position,
        &part_id,
        &site_num,
        &path,
    )
}

#[tauri::command(async)]
fn get_record_groups(
    session_id: String,
    manager: State<'_, SessionManager>,
) -> Result<Vec<RecordGroup>, String> {
    manager.get_record_groups(&session_id)
}

#[tauri::command(async)]
fn get_records(
    session_id: String,
    group: String,
    page: usize,
    page_size: usize,
    manager: State<'_, SessionManager>,
) -> Result<RecordSummaryPage, String> {
    manager.get_records(&session_id, &group, page, page_size)
}

#[tauri::command(async)]
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
    manager.search_fields(
        &session_id,
        &query,
        page,
        page_size,
        move |scanned, total| {
            let _ = on_progress.send(SearchProgress {
                session_id: sid.clone(),
                scanned,
                total,
            });
        },
    )
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
            get_test_item_page,
            get_test_item_columns,
            get_bin_summary,
            export_test_item_csv,
            parse_dtr_text,
            save_dtr_text,
            parse_gdr_text,
            save_gdr_text,
            get_mpr_pin_details,
            export_mpr_pins_xlsx,
            get_record_groups,
            get_records,
            get_record_fields,
            search_fields
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
