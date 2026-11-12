//! Native account and process boundaries. ACP and management contracts stay portable.
pub fn auth_method() -> &'static str {
    if cfg!(windows) {
        "windows_logon"
    } else {
        "pam"
    }
}

pub fn owner_matches(owner: &str, supplied: &str) -> bool {
    if cfg!(windows) {
        owner.eq_ignore_ascii_case(supplied)
            || owner
                .rsplit_once('\\')
                .is_some_and(|(_, user)| user.eq_ignore_ascii_case(supplied))
    } else {
        owner == supplied
    }
}

pub fn python() -> &'static str {
    if cfg!(windows) { "python" } else { "python3" }
}

/// Consumer installations carry their interpreter. A damaged private copy is
/// an installation error, never permission to silently use another Python.
pub fn installation_python(home: &std::path::Path) -> anyhow::Result<std::path::PathBuf> {
    let lock: serde_json::Value =
        serde_json::from_str(include_str!("../scripts/vendor/python/lock.json"))?;
    let root = home
        .join("support")
        .join(lock["directory"].as_str().unwrap());
    anyhow::ensure!(
        !root.is_symlink(),
        "The installation's private Python directory must not be a link"
    );
    if root.exists() {
        let executable = root.join(if cfg!(windows) {
            "python.exe"
        } else {
            "bin/python3.13"
        });
        anyhow::ensure!(
            executable.is_file(),
            "The installation's private Python is incomplete; repair its consumer bundle"
        );
        return Ok(executable);
    }
    // Bare developer executables and established installations retain their
    // existing setup. Consumer setup always stages the private copy first.
    Ok(python().into())
}

pub fn executable(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

/// Keep these files open for the lifetime of the service. Only explicitly
/// requested native startup jobs redirect; stdio ACP connections never do.
pub fn redirect_logs(directory: &std::path::Path) -> anyhow::Result<Vec<std::fs::File>> {
    crate::config::private_dir(directory)?;
    let mut files = Vec::new();
    for (name, stream) in [("service.stdout.log", 1), ("service.stderr.log", 2)] {
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(directory.join(name))?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            anyhow::ensure!(
                unsafe { libc::dup2(file.as_raw_fd(), stream) } >= 0,
                "Cannot open service log stream"
            );
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::System::Console::{
                STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
            };
            let stream = if stream == 1 {
                STD_OUTPUT_HANDLE
            } else {
                STD_ERROR_HANDLE
            };
            anyhow::ensure!(
                unsafe { SetStdHandle(stream, file.as_raw_handle()) } != 0,
                "Cannot open service log stream"
            );
        }
        files.push(file);
    }
    Ok(files)
}

#[cfg(target_os = "macos")]
pub mod macos {
    // macOS ships OpenPAM. pam-client's Linux constants/signatures are not
    // compatible; keep this small ABI surface matched to Apple's pam_appl.h.
    use std::{
        ffi::{CStr, CString, c_char, c_int, c_void},
        ptr,
    };
    use zeroize::Zeroizing;
    #[repr(C)]
    struct Message {
        style: c_int,
        text: *mut c_char,
    }
    #[repr(C)]
    struct Response {
        text: *mut c_char,
        code: c_int,
    }
    #[repr(C)]
    struct Conversation {
        callback: unsafe extern "C" fn(
            c_int,
            *mut *const Message,
            *mut *mut Response,
            *mut c_void,
        ) -> c_int,
        data: *mut c_void,
    }
    #[link(name = "pam")]
    unsafe extern "C" {
        fn pam_start(
            service: *const c_char,
            user: *const c_char,
            conversation: *const Conversation,
            handle: *mut *mut c_void,
        ) -> c_int;
        fn pam_authenticate(handle: *mut c_void, flags: c_int) -> c_int;
        fn pam_acct_mgmt(handle: *mut c_void, flags: c_int) -> c_int;
        fn pam_end(handle: *mut c_void, status: c_int) -> c_int;
    }
    struct Password {
        bytes: Zeroizing<Vec<u8>>,
        answered: bool,
    }
    unsafe extern "C" fn conversation(
        count: c_int,
        messages: *mut *const Message,
        output: *mut *mut Response,
        data: *mut c_void,
    ) -> c_int {
        const CONV_ERR: c_int = 6;
        if !(1..=32).contains(&count) || messages.is_null() || output.is_null() || data.is_null() {
            return CONV_ERR;
        }
        unsafe {
            *output = ptr::null_mut();
        }
        let password = unsafe { &mut *data.cast::<Password>() };
        let responses =
            unsafe { libc::calloc(count as usize, size_of::<Response>()).cast::<Response>() };
        if responses.is_null() {
            return 5;
        }
        let mut valid = true;
        for index in 0..count as usize {
            let message = unsafe { *messages.add(index) };
            if message.is_null() {
                valid = false;
                break;
            }
            let message = unsafe { &*message };
            match message.style {
                1 if !password.answered
                    && !message.text.is_null()
                    && unsafe { CStr::from_ptr(message.text) }
                        .to_string_lossy()
                        .trim()
                        .eq_ignore_ascii_case("password:") =>
                {
                    password.answered = true;
                    unsafe {
                        (*responses.add(index)).text = libc::strdup(password.bytes.as_ptr().cast());
                    }
                    if unsafe { (*responses.add(index)).text.is_null() } {
                        valid = false;
                        break;
                    }
                }
                3 | 4 => {} // Informational text; never supply a password as an answer.
                _ => {
                    valid = false;
                    break;
                }
            }
        }
        if valid {
            // OpenPAM owns and frees the C-allocated replies on success.
            unsafe {
                *output = responses;
            }
            0
        } else {
            for index in 0..count as usize {
                unsafe {
                    libc::free((*responses.add(index)).text.cast());
                }
            }
            unsafe {
                libc::free(responses.cast());
            }
            CONV_ERR
        }
    }
    pub fn authenticate(service: &str, username: &str, password: String) -> bool {
        let password = Zeroizing::new(password);
        if password.contains('\0') {
            return false;
        }
        let (Ok(service), Ok(username)) = (CString::new(service), CString::new(username)) else {
            return false;
        };
        let mut bytes = Zeroizing::new(password.as_bytes().to_vec());
        bytes.push(0);
        let mut data = Password {
            bytes,
            answered: false,
        };
        let conversation = Conversation {
            callback: conversation,
            data: (&mut data as *mut Password).cast(),
        };
        let mut handle = ptr::null_mut();
        let mut status = unsafe {
            pam_start(
                service.as_ptr(),
                username.as_ptr(),
                &conversation,
                &mut handle,
            )
        };
        if status == 0 {
            status = unsafe { pam_authenticate(handle, 0) };
            if status == 0 {
                status = unsafe { pam_acct_mgmt(handle, 0) };
            }
        }
        if !handle.is_null() {
            unsafe {
                pam_end(handle, status);
            }
        }
        status == 0
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn prompt_refuses_mfa_and_repeated_passwords() {
            let mut password = Password {
                bytes: Zeroizing::new(b"fixture\0".to_vec()),
                answered: false,
            };
            let mut response = ptr::null_mut();
            for (prompt, expected) in [(c"One-time code:", 6), (c"Password:", 0), (c"Password:", 6)]
            {
                let message = Message {
                    style: 1,
                    text: prompt.as_ptr().cast_mut(),
                };
                let mut pointer = &message as *const Message;
                let code = unsafe {
                    conversation(
                        1,
                        &mut pointer,
                        &mut response,
                        (&mut password as *mut Password).cast(),
                    )
                };
                assert_eq!(code, expected);
                if code == 0 {
                    assert_eq!(
                        unsafe { CStr::from_ptr((*response).text) }.to_bytes(),
                        b"fixture"
                    );
                    unsafe {
                        libc::free((*response).text.cast());
                        libc::free(response.cast());
                    }
                }
            }
        }
        #[test]
        fn missing_pam_policy_fails_closed() {
            assert!(!authenticate(
                "agent-instance-nonexistent-test-policy",
                "agent-instance-no-such-user",
                "fixture".into()
            ));
        }
    }
}

#[cfg(windows)]
pub mod windows {
    use anyhow::{Context, Result, ensure};
    use std::{ffi::c_void, os::windows::ffi::OsStrExt, path::Path, ptr};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE, LocalFree},
        Security::{
            Authentication::Identity::{GetUserNameExW, NameSamCompatible},
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                SDDL_REVISION_1, SE_FILE_OBJECT, SetNamedSecurityInfoW,
            },
            DACL_SECURITY_INFORMATION, EqualSid, GetSecurityDescriptorDacl, GetTokenInformation,
            LOGON32_LOGON_NETWORK, LOGON32_PROVIDER_DEFAULT, LogonUserW,
            PROTECTED_DACL_SECURITY_INFORMATION, TOKEN_QUERY, TOKEN_USER, TokenUser,
        },
        System::{
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                SetInformationJobObject,
            },
            Threading::{
                GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_SET_QUOTA,
                PROCESS_TERMINATE,
            },
        },
    };
    use zeroize::Zeroizing;

    struct OwnedHandle(HANDLE);
    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }
    fn process_token() -> Result<OwnedHandle> {
        let mut token = ptr::null_mut();
        ensure!(
            unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } != 0,
            "Cannot inspect the service's OS identity: {}",
            std::io::Error::last_os_error()
        );
        Ok(OwnedHandle(token))
    }
    fn token_user(token: HANDLE) -> Result<Vec<usize>> {
        let mut size = 0;
        unsafe {
            GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut size);
        }
        ensure!(size > 0 && size < 65536, "Invalid OS identity size");
        // Pointer-aligned storage contains TOKEN_USER and its referenced SID.
        let mut data = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
        ensure!(
            unsafe {
                GetTokenInformation(token, TokenUser, data.as_mut_ptr().cast(), size, &mut size)
            } != 0,
            "Cannot inspect OS identity"
        );
        Ok(data)
    }
    pub fn current_user() -> Result<String> {
        let mut size = 0;
        unsafe {
            GetUserNameExW(NameSamCompatible, ptr::null_mut(), &mut size);
        }
        ensure!(
            size > 0 && size < 32768,
            "Cannot identify current Windows account"
        );
        let mut data = vec![0u16; size as usize];
        ensure!(
            unsafe { GetUserNameExW(NameSamCompatible, data.as_mut_ptr(), &mut size) },
            "Cannot identify current Windows account"
        );
        String::from_utf16(&data[..size as usize]).context("Invalid Windows account name")
    }
    pub fn authenticate(username: &str, password: String) -> bool {
        // Passwords never reach a command line, environment, file or log.
        let password = Zeroizing::new(password);
        if username.contains('\0') || password.contains('\0') {
            return false;
        }
        let (domain, user) = username.split_once('\\').unwrap_or((".", username));
        let domain = wide(domain);
        let user = wide(user);
        let secret = Zeroizing::new(wide(&password));
        let mut token = ptr::null_mut();
        if unsafe {
            LogonUserW(
                user.as_ptr(),
                domain.as_ptr(),
                secret.as_ptr(),
                LOGON32_LOGON_NETWORK,
                LOGON32_PROVIDER_DEFAULT,
                &mut token,
            )
        } == 0
        {
            return false;
        }
        let token = OwnedHandle(token);
        // Name matching in the HTTP handler is supplemented by SID equality.
        let Ok(current) = process_token() else {
            return false;
        };
        let (Ok(owner), Ok(candidate)) = (token_user(current.0), token_user(token.0)) else {
            return false;
        };
        unsafe {
            EqualSid(
                (*(owner.as_ptr().cast::<TOKEN_USER>())).User.Sid,
                (*(candidate.as_ptr().cast::<TOKEN_USER>())).User.Sid,
            ) != 0
        }
    }
    pub fn private_dir(path: &Path) -> Result<()> {
        let token = process_token()?;
        let owner = token_user(token.0)?;
        let mut text = ptr::null_mut();
        ensure!(
            unsafe {
                ConvertSidToStringSidW((*(owner.as_ptr().cast::<TOKEN_USER>())).User.Sid, &mut text)
            } != 0,
            "Cannot resolve the installation owner's SID"
        );
        let mut length = 0;
        unsafe {
            while *text.add(length) != 0 {
                length += 1;
            }
        }
        let sid = unsafe { String::from_utf16_lossy(std::slice::from_raw_parts(text, length)) };
        unsafe {
            LocalFree(text.cast());
        }
        let descriptor_text = wide(&format!("D:P(A;OICI;FA;;;{sid})(A;OICI;FA;;;SY)"));
        let mut descriptor = ptr::null_mut::<c_void>();
        ensure!(
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    descriptor_text.as_ptr(),
                    SDDL_REVISION_1,
                    &mut descriptor,
                    ptr::null_mut(),
                )
            } != 0,
            "Cannot construct private installation permissions"
        );
        let mut dacl = ptr::null_mut();
        let mut present = 0;
        let mut defaulted = 0;
        let path: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let result = unsafe {
            if GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted) == 0
                || present == 0
            {
                1
            } else {
                SetNamedSecurityInfoW(
                    path.as_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    dacl,
                    ptr::null_mut(),
                )
            }
        };
        unsafe {
            LocalFree(descriptor);
        }
        ensure!(
            result == 0,
            "Cannot make installation folder private (Windows error {result})"
        );
        Ok(())
    }

    /// A kernel job owns this ACP process tree. Closing its last handle stops descendants.
    pub struct ProcessJob(usize);
    impl ProcessJob {
        pub fn attach(pid: u32) -> Result<Self> {
            let job = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
            ensure!(!job.is_null(), "Cannot create ACP process job");
            let guard = Self(job as usize);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            ensure!(
                unsafe {
                    SetInformationJobObject(
                        job,
                        JobObjectExtendedLimitInformation,
                        (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                        size_of_val(&limits) as u32,
                    )
                } != 0,
                "Cannot configure ACP process cleanup"
            );
            let process = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid) };
            ensure!(!process.is_null(), "Cannot attach ACP process to its job");
            let process = OwnedHandle(process);
            ensure!(
                unsafe { AssignProcessToJobObject(job, process.0) } != 0,
                "Cannot attach ACP process to its job: {}",
                std::io::Error::last_os_error()
            );
            Ok(guard)
        }
    }
    impl Drop for ProcessJob {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0 as HANDLE);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn current_identity_is_resolved_and_unknown_account_fails_closed() {
            let owner = current_user().unwrap();
            assert!(owner.contains('\\'));
            assert!(crate::platform::owner_matches(
                &owner,
                owner.rsplit_once('\\').unwrap().1
            ));
            assert!(!crate::platform::owner_matches(
                &owner,
                "different-domain\\nobody"
            ));
            assert!(!authenticate(
                ".\\agent-instance-nonexistent-test-account",
                "fixture-only".into()
            ));
        }
    }
}

#[cfg(test)]
mod private_python_tests {
    use super::*;

    #[test]
    fn private_runtime_is_preferred_and_incomplete_copy_does_not_fall_back() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(
            installation_python(home.path()).unwrap(),
            std::path::PathBuf::from(python())
        );
        let root = home.path().join("support/python-3.13.18-20261110");
        std::fs::create_dir_all(&root).unwrap();
        assert!(installation_python(home.path()).is_err());
        let executable = root.join(if cfg!(windows) {
            "python.exe"
        } else {
            "bin/python3.13"
        });
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::write(&executable, b"fixture only").unwrap();
        assert_eq!(installation_python(home.path()).unwrap(), executable);
    }
}
