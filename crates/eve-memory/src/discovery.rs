//! Game client discovery: find all running EVE Online clients (any flavor),
//! identify their server edition and character, and attach their window.

use std::path::PathBuf;

use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
    TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::core::PWSTR;

use crate::error::{Error, Result};
use crate::flavor::{parse_window_title, Flavor};
use crate::window::{enumerate_titled_windows, WindowHandle};

/// Executable name of the EVE Online client (64-bit, all publishers).
pub const CLIENT_PROCESS_NAME: &str = "exefile.exe";

/// One running game client instance.
#[derive(Clone, Debug)]
pub struct GameClient {
    pub pid: u32,
    pub exe_path: PathBuf,
    /// Main (game) window handle, if any top-level window exists.
    pub window: Option<WindowHandle>,
    pub window_title: String,
    pub flavor: Flavor,
    pub character_name: Option<String>,
}

impl GameClient {
    /// Attach to the process for memory reading.
    pub fn open_live(&self) -> Result<crate::source::LiveProcess> {
        crate::source::LiveProcess::open(self.pid)
    }

    pub fn window_handle_raw(&self) -> Option<isize> {
        self.window.map(|w| w.raw())
    }
}

/// Discover all running EVE Online clients.
///
/// Every `exefile.exe` process becomes a [`GameClient`]; the flavor is taken
/// from the window title (`[Infinity]` / `[Serenity]` / `[Tranquility]`)
/// with the install path as fallback.
pub fn discover_clients() -> Result<Vec<GameClient>> {
    let pids = snapshot_process_ids_by_name(CLIENT_PROCESS_NAME);
    if pids.is_empty() {
        return Err(Error::NoClientProcess);
    }
    let windows = enumerate_titled_windows();
    let mut clients = Vec::with_capacity(pids.len());
    for pid in pids {
        let Some(exe_path) = query_process_image_path(pid) else {
            continue;
        };
        // Prefer a window whose title carries a server tag; fall back to the
        // first titled window of the process.
        let mut best: Option<(WindowHandle, String)> = None;
        for (handle, window_pid, title) in &windows {
            if *window_pid != pid {
                continue;
            }
            let title_flavor = Flavor::from_window_title(title);
            let better = match &best {
                None => true,
                Some((_, best_title)) => {
                    Flavor::from_window_title(best_title) == Flavor::Unknown
                        && title_flavor != Flavor::Unknown
                }
            };
            if better {
                best = Some((*handle, title.clone()));
            }
        }
        let (window, window_title) = match best {
            Some((handle, title)) => (Some(handle), title),
            None => (None, String::new()),
        };
        let title_info = parse_window_title(&window_title);
        let flavor = match title_info.flavor {
            Flavor::Unknown => Flavor::from_install_path(&exe_path),
            flavor => flavor,
        };
        clients.push(GameClient {
            pid,
            exe_path,
            window,
            window_title,
            flavor,
            character_name: title_info.character_name,
        });
    }
    Ok(clients)
}

fn snapshot_process_ids_by_name(name: &str) -> Vec<u32> {
    let mut pids = Vec::new();
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return pids;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let exe = String::from_utf16_lossy(
                    &entry.szExeFile[..entry
                        .szExeFile
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(entry.szExeFile.len())],
                );
                if exe.eq_ignore_ascii_case(name) {
                    pids.push(entry.th32ProcessID);
                }
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = windows::Win32::Foundation::CloseHandle(snapshot);
    }
    pids
}

fn query_process_image_path(pid: u32) -> Option<PathBuf> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 1024];
        let mut size = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut size,
        );
        let _ = windows::Win32::Foundation::CloseHandle(handle);
        if result.is_err() {
            return None;
        }
        let path = String::from_utf16_lossy(&buffer[..size as usize]);
        Some(PathBuf::from(path))
    }
}
