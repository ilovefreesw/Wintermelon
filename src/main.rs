#[cfg(windows)]
mod app {
    use std::collections::HashMap;
    use std::error::Error;
    use std::ffi::{c_void, OsString};
    use std::io;
    use std::mem::size_of;
    use std::os::windows::ffi::OsStringExt;
    use std::path::PathBuf;
    use std::ptr;

    type Hwnd = *mut c_void;
    type Hmonitor = *mut c_void;
    type Bool = i32;

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const MONITOR_DEFAULTTONEAREST: u32 = 2;
    const MONITORINFOF_PRIMARY: u32 = 1;
    const DWMWA_CLOAKED: u32 = 14;
    const SDC_TOPOLOGY_CLONE: u32 = 0x2;
    const SDC_TOPOLOGY_EXTEND: u32 = 0x4;
    const SDC_APPLY: u32 = 0x80;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct WindowPlacement {
        length: u32,
        flags: u32,
        show_cmd: u32,
        min_position: Point,
        max_position: Point,
        normal_position: Rect,
    }

    #[repr(C)]
    struct MonitorInfoExW {
        cb_size: u32,
        monitor: Rect,
        work: Rect,
        flags: u32,
        device: [u16; 32],
    }

    struct WindowSnapshot {
        handle: Hwnd,
        pid: u32,
        title: String,
        placement: WindowPlacement,
        primary: bool,
        device: String,
        executable: Option<PathBuf>,
    }

    struct Enumeration {
        windows: Vec<WindowSnapshot>,
        error: Option<io::Error>,
    }

    #[link(name = "user32")]
    extern "system" {
        fn EnumDesktopWindows(
            desktop: Hwnd,
            callback: Option<unsafe extern "system" fn(Hwnd, isize) -> Bool>,
            lparam: isize,
        ) -> Bool;
        fn IsWindowVisible(hwnd: Hwnd) -> Bool;
        fn GetWindowTextW(hwnd: Hwnd, text: *mut u16, max_count: i32) -> i32;
        fn GetWindowPlacement(hwnd: Hwnd, placement: *mut WindowPlacement) -> Bool;
        fn SetWindowPlacement(hwnd: Hwnd, placement: *const WindowPlacement) -> Bool;
        fn GetWindowThreadProcessId(hwnd: Hwnd, process_id: *mut u32) -> u32;
        fn SetDisplayConfig(
            path_count: u32,
            paths: *mut c_void,
            mode_count: u32,
            modes: *mut c_void,
            flags: u32,
        ) -> i32;
        fn MonitorFromWindow(hwnd: Hwnd, flags: u32) -> Hmonitor;
        fn GetMonitorInfoW(monitor: Hmonitor, info: *mut c_void) -> Bool;
    }

    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmGetWindowAttribute(hwnd: Hwnd, attribute: u32, value: *mut c_void, size: u32) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit_handle: Bool, process_id: u32) -> Hwnd;
        fn QueryFullProcessImageNameW(
            process: Hwnd,
            flags: u32,
            path: *mut u16,
            size: *mut u32,
        ) -> Bool;
        fn CloseHandle(handle: Hwnd) -> Bool;
    }

    pub fn run() -> Result<(), Box<dyn Error>> {
        let mut args = std::env::args_os().skip(1);
        let Some(command) = args.next() else {
            return Ok(());
        };
        let command = command.to_string_lossy();
        if !command.eq_ignore_ascii_case("save") && !command.eq_ignore_ascii_case("restore") {
            return Ok(());
        }

        let local_app_data = std::env::var_os("LOCALAPPDATA")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "LOCALAPPDATA is not set"))?;
        let app_dir = PathBuf::from(local_app_data).join("Wintermelon");
        std::fs::create_dir_all(&app_dir)?;
        let settings_file = app_dir.join("winpos.txt");
        let windows = enumerate_windows()?;

        if command.eq_ignore_ascii_case("save") {
            let contents = windows
                .iter()
                .filter_map(serialize_window)
                .collect::<Vec<_>>()
                .join("\n");
            std::fs::write(&settings_file, contents)?;
            set_display_topology(SDC_TOPOLOGY_CLONE)?;
        } else {
            set_display_topology(SDC_TOPOLOGY_EXTEND)?;
            restore_windows(&settings_file, &windows)?;
        }

        Ok(())
    }

    fn enumerate_windows() -> io::Result<Vec<WindowSnapshot>> {
        let mut enumeration = Enumeration {
            windows: Vec::new(),
            error: None,
        };
        let succeeded = unsafe {
            EnumDesktopWindows(
                ptr::null_mut(),
                Some(enumerate_window),
                &mut enumeration as *mut Enumeration as isize,
            )
        };

        if let Some(error) = enumeration.error {
            return Err(error);
        }
        if succeeded == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(enumeration.windows)
    }

    unsafe extern "system" fn enumerate_window(hwnd: Hwnd, lparam: isize) -> Bool {
        let enumeration = unsafe { &mut *(lparam as *mut Enumeration) };
        if unsafe { IsWindowVisible(hwnd) } == 0 {
            return 1;
        }

        let mut title_buffer = [0u16; 256];
        let title_len =
            unsafe { GetWindowTextW(hwnd, title_buffer.as_mut_ptr(), title_buffer.len() as i32) };
        if title_len == 0 {
            return 1;
        }
        let title = String::from_utf16_lossy(&title_buffer[..title_len as usize]);

        let mut placement = WindowPlacement {
            length: size_of::<WindowPlacement>() as u32,
            ..WindowPlacement::default()
        };
        if unsafe { GetWindowPlacement(hwnd, &mut placement) } == 0 {
            enumeration.error = Some(io::Error::last_os_error());
            return 0;
        }

        let mut cloaked = 0u32;
        let cloak_result = unsafe {
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED,
                &mut cloaked as *mut u32 as *mut c_void,
                size_of::<u32>() as u32,
            )
        };
        if cloak_result >= 0 && cloaked != 0 {
            return 1;
        }

        let mut pid = 0;
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut pid);
        }
        if pid == 0 {
            return 1;
        }

        let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
        if monitor.is_null() {
            enumeration.error = Some(io::Error::last_os_error());
            return 0;
        }
        let mut monitor_info = MonitorInfoExW {
            cb_size: size_of::<MonitorInfoExW>() as u32,
            monitor: Rect::default(),
            work: Rect::default(),
            flags: 0,
            device: [0; 32],
        };
        if unsafe {
            GetMonitorInfoW(
                monitor,
                &mut monitor_info as *mut MonitorInfoExW as *mut c_void,
            )
        } == 0
        {
            enumeration.error = Some(io::Error::last_os_error());
            return 0;
        }

        let device_len = monitor_info
            .device
            .iter()
            .position(|character| *character == 0)
            .unwrap_or(monitor_info.device.len());
        let device = String::from_utf16_lossy(&monitor_info.device[..device_len]);
        let executable = process_image_path(pid);

        enumeration.windows.push(WindowSnapshot {
            handle: hwnd,
            pid,
            title,
            placement,
            primary: monitor_info.flags & MONITORINFOF_PRIMARY != 0,
            device,
            executable,
        });
        1
    }

    fn process_image_path(pid: u32) -> Option<PathBuf> {
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if process.is_null() {
            return None;
        }

        let mut path = vec![0u16; 32_768];
        let mut length = path.len() as u32;
        let succeeded =
            unsafe { QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut length) };
        unsafe {
            CloseHandle(process);
        }
        if succeeded == 0 {
            return None;
        }
        path.truncate(length as usize);
        Some(PathBuf::from(OsString::from_wide(&path)))
    }

    fn serialize_window(window: &WindowSnapshot) -> Option<String> {
        let executable = window.executable.as_ref()?;
        if !executable
            .extension()
            .is_some_and(|extension| extension.to_string_lossy().eq_ignore_ascii_case("exe"))
        {
            return None;
        }
        let placement = window.placement;
        Some(format!(
            "Title={}\tPID={}\tID={}\tName={}\tLength={}\tFlags={}\tCmd={}\tMin=({}, {})\tMax=({}, {})\tRect=({},{},{},{})\tPrimary={}\tDisplay={}",
            window.title,
            window.pid,
            window.handle as isize,
            executable.display(),
            placement.length,
            placement.flags,
            placement.show_cmd,
            placement.min_position.x,
            placement.min_position.y,
            placement.max_position.x,
            placement.max_position.y,
            placement.normal_position.left,
            placement.normal_position.top,
            placement.normal_position.right,
            placement.normal_position.bottom,
            if window.primary { "True" } else { "False" },
            window.device,
        ))
    }

    fn restore_windows(
        settings_file: &std::path::Path,
        windows: &[WindowSnapshot],
    ) -> Result<(), Box<dyn Error>> {
        let contents = std::fs::read_to_string(settings_file)?;
        let mut saved = HashMap::new();
        for (index, line) in contents.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            if let Some((key, placement)) = parse_saved_window(line)
                .map_err(|error| format!("invalid settings at line {}: {error}", index + 1))?
            {
                saved.insert(key, placement);
            }
        }

        for window in windows {
            let key = window_key(window.pid, window.handle, &window.title);
            let Some(placement) = saved.get(&key) else {
                continue;
            };

            let mut normal = *placement;
            normal.length = size_of::<WindowPlacement>() as u32;
            normal.show_cmd = 1;
            set_window_placement(window.handle, &normal)?;

            let mut original = *placement;
            original.length = size_of::<WindowPlacement>() as u32;
            set_window_placement(window.handle, &original)?;
        }
        Ok(())
    }

    fn parse_saved_window(line: &str) -> Result<Option<(String, WindowPlacement)>, String> {
        let mut fields = HashMap::new();
        for field in line.split('\t') {
            if let Some((key, value)) = field.split_once('=') {
                fields.insert(key, value);
            }
        }

        let required = |key: &str| {
            fields
                .get(key)
                .copied()
                .ok_or_else(|| format!("missing {key} field"))
        };
        let pid = required("PID")?;
        let id = required("ID")?;
        let title = required("Title")?;
        let primary = required("Primary")?;
        if primary.eq_ignore_ascii_case("true") {
            return Ok(None);
        }

        let flags = required("Flags")?
            .parse::<u32>()
            .map_err(|error| format!("invalid Flags value: {error}"))?;
        let show_cmd = required("Cmd")?
            .parse::<u32>()
            .map_err(|error| format!("invalid Cmd value: {error}"))?;
        let min = parse_coordinates(required("Min")?, 2)?;
        let max = parse_coordinates(required("Max")?, 2)?;
        let rect = parse_coordinates(required("Rect")?, 4)?;

        Ok(Some((
            format!("{pid}_{id}_{title}"),
            WindowPlacement {
                length: size_of::<WindowPlacement>() as u32,
                flags,
                show_cmd,
                min_position: Point {
                    x: min[0],
                    y: min[1],
                },
                max_position: Point {
                    x: max[0],
                    y: max[1],
                },
                normal_position: Rect {
                    left: rect[0],
                    top: rect[1],
                    right: rect[2],
                    bottom: rect[3],
                },
            },
        )))
    }

    fn parse_coordinates(value: &str, expected: usize) -> Result<Vec<i32>, String> {
        let value = value
            .strip_prefix('(')
            .and_then(|value| value.strip_suffix(')'))
            .ok_or_else(|| format!("invalid coordinates: {value}"))?;
        let coordinates = value
            .split(',')
            .map(|coordinate| {
                coordinate
                    .trim()
                    .parse::<i32>()
                    .map_err(|error| format!("invalid coordinate: {error}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if coordinates.len() != expected {
            return Err(format!(
                "expected {expected} coordinates, found {}",
                coordinates.len()
            ));
        }
        Ok(coordinates)
    }

    fn window_key(pid: u32, handle: Hwnd, title: &str) -> String {
        format!("{pid}_{}_{title}", handle as isize)
    }

    fn set_window_placement(hwnd: Hwnd, placement: &WindowPlacement) -> io::Result<()> {
        if unsafe { SetWindowPlacement(hwnd, placement) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn set_display_topology(topology: u32) -> io::Result<()> {
        let result = unsafe {
            SetDisplayConfig(0, ptr::null_mut(), 0, ptr::null_mut(), SDC_APPLY | topology)
        };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(result))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{parse_saved_window, WindowPlacement};
        use std::mem::size_of;

        #[test]
        fn reads_legacy_saved_window_format() {
            let record = "Title=Editor=One\tPID=42\tID=1234\tName=C:\\Apps\\editor.exe\tLength=44\tFlags=0\tCmd=3\tMin=(1, 2)\tMax=(3, 4)\tRect=(-1920,0,0,1080)\tPrimary=False\tDisplay=\\\\.\\DISPLAY2";
            let (key, placement) = parse_saved_window(record).unwrap().unwrap();

            assert_eq!(key, "42_1234_Editor=One");
            assert_eq!(placement.length, size_of::<WindowPlacement>() as u32);
            assert_eq!(placement.show_cmd, 3);
            assert_eq!(placement.normal_position.left, -1920);
            assert_eq!(placement.normal_position.bottom, 1080);
        }

        #[test]
        fn ignores_primary_display_records() {
            let record = "Title=Editor\tPID=42\tID=1234\tFlags=0\tCmd=1\tMin=(0,0)\tMax=(0,0)\tRect=(0,0,1,1)\tPrimary=True";
            assert!(parse_saved_window(record).unwrap().is_none());
        }
    }
}

#[cfg(windows)]
fn main() {
    if let Err(error) = app::run() {
        eprintln!("Wintermelon: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("Wintermelon only supports Windows.");
    std::process::exit(1);
}
