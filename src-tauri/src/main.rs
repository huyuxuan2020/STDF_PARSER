// Windows: 以「窗口程序」启动，避免 release 版运行时弹出黑色 cmd 控制台窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    stdf_viewer_mac_lib::run();
}
