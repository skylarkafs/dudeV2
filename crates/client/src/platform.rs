use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use nextdude_common::{APP_NAME, APP_USER_MODEL_ID};

pub struct SingleInstanceGuard {
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            if !self.handle.is_null() {
                // SAFETY: `self.handle` is a valid Win32 mutex handle acquired from CreateMutexW.
                unsafe {
                    windows_sys::Win32::Foundation::CloseHandle(self.handle);
                }
            }
        }
    }
}

pub fn acquire_single_instance_mutex() -> Result<SingleInstanceGuard, String> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
        use windows_sys::Win32::System::Threading::CreateMutexW;
        let name_wide = to_wide_null("Local\\NextDudeClientSingleInstanceMutex");
        // SAFETY: `name_wide` is a valid null-terminated UTF-16 buffer for the duration of the call.
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name_wide.as_ptr()) };
        if handle.is_null() {
            return Err("CreateMutexW failed".to_string());
        }
        // SAFETY: `GetLastError` reads thread-local Win32 error code immediately after CreateMutexW.
        let err = unsafe { GetLastError() };
        if err == ERROR_ALREADY_EXISTS {
            // SAFETY: Close duplicated handle before returning already-running error.
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(handle);
            }
            return Err("Another instance of NextDude Client is already running".to_string());
        }
        Ok(SingleInstanceGuard { handle })
    }
    #[cfg(not(windows))]
    {
        Ok(SingleInstanceGuard {})
    }
}

pub fn register_app_user_model_id() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
        let wide = to_wide_null(APP_USER_MODEL_ID);
        // SAFETY: `wide` is null-terminated UTF-16 and lives across the synchronous Win32 call.
        unsafe {
            let _ = SetCurrentProcessExplicitAppUserModelID(wide.as_ptr());
        }
    }
}

pub fn set_prevent_sleep(prevent: bool) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Power::{
            SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED,
        };
        let flags = if prevent {
            ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED
        } else {
            ES_CONTINUOUS
        };
        // SAFETY: Pure Win32 thread state flag update with constant bitmask.
        unsafe {
            let _ = SetThreadExecutionState(flags);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = prevent;
    }
}

pub fn dpapi_protect_b64(plaintext: &str) -> Result<String, String> {
    if plaintext.is_empty() {
        return Ok(String::new());
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::Cryptography::{
            CryptProtectData, CRYPT_INTEGER_BLOB,
        };
        let bytes = plaintext.as_bytes();
        let data_in = CRYPT_INTEGER_BLOB {
            cbData: bytes.len() as u32,
            pbData: bytes.as_ptr().cast_mut(),
        };
        let mut data_out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        // SAFETY: `data_in` points to valid `bytes` for the synchronous DPAPI call, and `data_out`
        // receives an OS-allocated buffer that we copy and immediately release with `LocalFree`.
        let ok = unsafe {
            CryptProtectData(
                &data_in,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                0, // CurrentUser scope
                &mut data_out,
            )
        };
        if ok == 0 || data_out.pbData.is_null() {
            return Err("DPAPI CryptProtectData failed".to_string());
        }
        // SAFETY: `CryptProtectData` succeeded and guarantees `pbData` has `cbData` readable bytes.
        let sealed = unsafe {
            let slice = std::slice::from_raw_parts(data_out.pbData, data_out.cbData as usize);
            let out = slice.to_vec();
            let _ = LocalFree(data_out.pbData.cast());
            out
        };
        Ok(B64.encode(sealed))
    }
    #[cfg(not(windows))]
    {
        Err("Windows DPAPI is only available on Windows".to_string())
    }
}

pub fn dpapi_unprotect_b64(encoded: &str) -> Result<String, String> {
    if encoded.is_empty() {
        return Ok(String::new());
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::Cryptography::{
            CryptUnprotectData, CRYPT_INTEGER_BLOB,
        };
        let raw = B64.decode(encoded).map_err(|e| format!("invalid DPAPI b64: {e}"))?;
        let data_in = CRYPT_INTEGER_BLOB {
            cbData: raw.len() as u32,
            pbData: raw.as_ptr().cast_mut(),
        };
        let mut data_out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        // SAFETY: `data_in` points to valid `raw` bytes; `data_out` is freed with `LocalFree`.
        let ok = unsafe {
            CryptUnprotectData(
                &data_in,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                &mut data_out,
            )
        };
        if ok == 0 || data_out.pbData.is_null() {
            return Err("DPAPI CryptUnprotectData failed".to_string());
        }
        // SAFETY: `CryptUnprotectData` succeeded; `pbData` is valid for `cbData` bytes.
        let plain = unsafe {
            let slice = std::slice::from_raw_parts(data_out.pbData, data_out.cbData as usize);
            let out = slice.to_vec();
            let _ = LocalFree(data_out.pbData.cast());
            out
        };
        String::from_utf8(plain).map_err(|e| format!("DPAPI output not UTF-8: {e}"))
    }
    #[cfg(not(windows))]
    {
        let _ = encoded;
        Err("Windows DPAPI is only available on Windows".to_string())
    }
}

pub fn show_windows_toast(title: &str, body: &str) {
    #[cfg(windows)]
    {
        use windows::core::HSTRING;
        use windows::Data::Xml::Dom::XmlDocument;
        use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};

        let esc_title = xml_escape(title);
        let esc_body = xml_escape(body);
        let xml = format!(
            "<toast><visual><binding template=\"ToastGeneric\"><text>{esc_title}</text><text>{esc_body}</text></binding></visual></toast>"
        );
        if let Ok(doc) = XmlDocument::new() {
            if doc.LoadXml(&HSTRING::from(xml)).is_ok() {
                if let Ok(toast) = ToastNotification::CreateToastNotification(&doc) {
                    if let Ok(notifier) = ToastNotificationManager::CreateToastNotifierWithId(
                        &HSTRING::from(APP_USER_MODEL_ID),
                    ) {
                        let _ = notifier.Show(&toast);
                    }
                }
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (title, body);
    }
}

#[cfg(windows)]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Generates an in-memory 22050 Hz mono PCM WAV alarm tone and plays it asynchronously in a loop.
pub fn start_alarm_sound(wav_storage: &mut Option<Vec<u8>>) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Media::Audio::{
            PlaySoundW, SND_ASYNC, SND_LOOP, SND_MEMORY, SND_NODEFAULT,
        };
        if wav_storage.is_none() {
            *wav_storage = Some(build_alarm_wav_bytes());
        }
        if let Some(ref wav) = wav_storage {
            // SAFETY: `wav` lives inside `NextDudeApp` while playing; `stop_alarm_sound` stops
            // playback before dropping/mutating `wav_storage`.
            unsafe {
                let _ = PlaySoundW(
                    wav.as_ptr().cast(),
                    std::ptr::null_mut(),
                    SND_MEMORY | SND_ASYNC | SND_LOOP | SND_NODEFAULT,
                );
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = wav_storage;
    }
}

pub fn stop_alarm_sound() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Media::Audio::PlaySoundW;
        // SAFETY: Passing null stops any active asynchronous PlaySoundW playback on the process.
        unsafe {
            let _ = PlaySoundW(std::ptr::null(), std::ptr::null_mut(), 0);
        }
    }
}

pub fn set_autostart_registry(enable: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Registry::{
            RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
            KEY_SET_VALUE, REG_SZ,
        };
        let subkey = to_wide_null(r"Software\Microsoft\Windows\CurrentVersion\Run");
        let val_name = to_wide_null(APP_NAME);
        let mut hkey: HKEY = std::ptr::null_mut();
        // SAFETY: `subkey` is null-terminated UTF-16 and `hkey` receives the opened key handle.
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                KEY_SET_VALUE,
                &mut hkey,
            )
        };
        if status != 0 {
            return Err(format!("RegOpenKeyExW failed with code {status}"));
        }

        let res = if enable {
            let exe = std::env::current_exe()
                .map_err(|e| format!("current_exe failed: {e}"))?;
            let cmd = format!("\"{}\"", exe.display());
            let wide_cmd = to_wide_null(&cmd);
            let byte_len = (wide_cmd.len() * std::mem::size_of::<u16>()) as u32;
            // SAFETY: `hkey` is open, `val_name` and `wide_cmd` are valid null-terminated UTF-16.
            let set_st = unsafe {
                RegSetValueExW(
                    hkey,
                    val_name.as_ptr(),
                    0,
                    REG_SZ,
                    wide_cmd.as_ptr().cast(),
                    byte_len,
                )
            };
            if set_st == 0 {
                Ok(())
            } else {
                Err(format!("RegSetValueExW failed with code {set_st}"))
            }
        } else {
            // SAFETY: `hkey` is open and `val_name` is null-terminated UTF-16.
            let _ = unsafe { RegDeleteValueW(hkey, val_name.as_ptr()) };
            Ok(())
        };

        // SAFETY: Close valid registry key handle.
        unsafe {
            let _ = RegCloseKey(hkey);
        }
        res
    }
    #[cfg(not(windows))]
    {
        let _ = enable;
        Ok(())
    }
}

pub fn launch_external_command(template: &str, address: &str) -> Result<(), String> {
    let expanded = template.replace("{address}", address).trim().to_string();
    if expanded.is_empty() {
        return Err("Empty launcher command".to_string());
    }
    if expanded.starts_with("https://") || expanded.starts_with("http://") {
        #[cfg(windows)]
        {
            std::process::Command::new("cmd")
                .args(["/C", "start", "", &expanded])
                .spawn()
                .map_err(|e| format!("failed opening browser: {e}"))?;
            return Ok(());
        }
        #[cfg(not(windows))]
        {
            return Err(format!("Browser launch only supported on Windows: {expanded}"));
        }
    }
    let parts: Vec<&str> = expanded.split_whitespace().collect();
    let Some((prog, args)) = parts.split_first() else {
        return Err("Invalid launcher command".to_string());
    };
    std::process::Command::new(prog)
        .args(args)
        .spawn()
        .map_err(|e| format!("failed launching {prog}: {e}"))?;
    Ok(())
}

#[cfg(windows)]
fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn build_alarm_wav_bytes() -> Vec<u8> {
    let sample_rate = 22050u32;
    let total_samples = (sample_rate as usize) * 2; // 2-second loop
    let data_bytes = total_samples * 2;
    let mut wav = Vec::with_capacity(44 + data_bytes);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&((36 + data_bytes) as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // Mono
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data_bytes as u32).to_le_bytes());

    for i in 0..total_samples {
        let t = (i as f32) / (sample_rate as f32);
        let amp = if t < 0.35 || (t >= 0.5 && t < 0.85) {
            let freq = if t < 0.35 { 880.0 } else { 660.0 };
            ((2.0 * std::f32::consts::PI * freq * t).sin() * 9000.0) as i16
        } else {
            0i16
        };
        wav.extend_from_slice(&amp.to_le_bytes());
    }
    wav
}
