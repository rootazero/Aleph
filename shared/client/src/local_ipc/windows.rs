//! Windows account authentication for the explicit local Desktop endpoint.

use std::{
    ffi::c_void,
    io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::Path,
    ptr::null_mut,
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::windows::named_pipe::{ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions},
};
use windows_sys::Win32::{
    Foundation::{LocalFree, ERROR_INSUFFICIENT_BUFFER, ERROR_NO_TOKEN, HANDLE, WAIT_TIMEOUT},
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            GetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT,
        },
        GetTokenInformation, RevertToSelf, TokenUser, OWNER_SECURITY_INFORMATION, PSID,
        SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    },
    Storage::FileSystem::SECURITY_IDENTIFICATION,
    System::{
        Pipes::{GetNamedPipeServerProcessId, ImpersonateNamedPipeClient},
        Threading::{
            GetCurrentProcess, GetCurrentThread, OpenProcess, OpenProcessToken, OpenThreadToken,
            WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        },
    },
};

const AUTH_BYTE: u8 = 0;

pub fn validate_endpoint(endpoint: &Path) -> io::Result<()> {
    if let Some(name) = endpoint.to_str().and_then(|p| p.strip_prefix(r"\\.\pipe\")) {
        if !name.is_empty()
            && !name.ends_with(['.', ' '])
            && name.encode_utf16().count() + 9 <= 256
            && !name
                .chars()
                .any(|c| c.is_control() || matches!(c, '\\' | '/' | ':'))
        {
            return Ok(());
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        r"Desktop IPC requires an explicit local \\.\pipe\name",
    ))
}

pub struct PipeAccount(String);

impl PipeAccount {
    pub fn current() -> io::Result<Self> {
        // SAFETY: the current process pseudo-handle is always valid.
        process_account(unsafe { GetCurrentProcess() }).map(Self)
    }

    pub fn create(
        &self,
        endpoint: &Path,
        first: bool,
        max_instances: usize,
    ) -> io::Result<NamedPipeServer> {
        validate_endpoint(endpoint)?;
        // Do not inherit the default owner/DACL (which can include other identities).
        // The one account may also provision subsequent server instances.
        let sddl: Vec<u16> = format!("O:{sid}D:P(A;;GA;;;{sid})\0", sid = self.0)
            .encode_utf16()
            .collect();
        let mut descriptor = LocalAllocation(null_mut());
        // SAFETY: the UTF-16 input and all out-parameters are valid for this call.
        check(unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor.0,
                null_mut(),
            )
        })?;
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: 0,
        };
        // SAFETY: attributes and the kernel-allocated descriptor remain valid through CreateNamedPipeW.
        unsafe {
            ServerOptions::new()
                .first_pipe_instance(first)
                .max_instances(max_instances)
                .reject_remote_clients(true)
                .create_with_security_attributes_raw(endpoint, (&raw mut attributes).cast())
        }
    }

    /// Read only a bounded transport prelude before consulting the peer token.
    pub async fn authenticate_client(&self, stream: &mut NamedPipeServer) -> io::Result<()> {
        if stream.read_u8().await? != AUTH_BYTE || self.0 != impersonated_account(stream)? {
            return Err(account_rejected());
        }
        stream.write_u8(AUTH_BYTE).await
    }
}

pub async fn connect(endpoint: &Path) -> io::Result<NamedPipeClient> {
    validate_endpoint(endpoint)?;
    let account = PipeAccount::current()?;
    let mut stream = ClientOptions::new()
        .security_qos_flags(SECURITY_IDENTIFICATION)
        .open(endpoint)?;
    require_server_account(&stream, &account)?;
    stream.write_u8(AUTH_BYTE).await?;
    if stream.read_u8().await? != AUTH_BYTE {
        return Err(account_rejected());
    }
    Ok(stream)
}

fn require_server_account(stream: &NamedPipeClient, account: &PipeAccount) -> io::Result<()> {
    let pipe = stream.as_raw_handle();
    // PID lookup alone is insufficient: the creator could have exited and its PID
    // could already have been recycled. Bind the SID to the OPEN pipe object too.
    // A different ordinary account cannot set our user SID as owner of its pipe.
    let mut owner = null_mut();
    let mut descriptor = LocalAllocation(null_mut());
    // SAFETY: pipe is open, the two output pointers are valid, all other outputs are unused.
    let result = unsafe {
        GetSecurityInfo(
            pipe,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            null_mut(),
            null_mut(),
            &mut descriptor.0,
        )
    };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    if owner.is_null() || sid_text(owner)? != account.0 {
        return Err(account_rejected());
    }

    let mut pid = 0;
    // SAFETY: pipe remains open and pid is valid writable storage.
    check(unsafe { GetNamedPipeServerProcessId(pipe, &mut pid) })?;
    // SAFETY: OpenProcess checks the PID; successful handles belong to this function.
    let process = owned_handle(unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        )
    })?;
    // Holding this handle pins the process object through both queries.
    let mut confirmed_pid = 0;
    if process_account(process.as_raw_handle())? != account.0
        // SAFETY: both handles are still open and confirmed_pid is writable.
        || unsafe { GetNamedPipeServerProcessId(pipe, &mut confirmed_pid) } == 0
        || confirmed_pid != pid
        || unsafe { WaitForSingleObject(process.as_raw_handle(), 0) } != WAIT_TIMEOUT
    {
        return Err(account_rejected());
    }
    Ok(())
}

fn process_account(process: HANDLE) -> io::Result<String> {
    let mut token = null_mut();
    // SAFETY: the caller supplies an open process handle or the current-process pseudo-handle.
    check(unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) })?;
    token_account(owned_handle(token)?)
}

fn impersonated_account(stream: &NamedPipeServer) -> io::Result<String> {
    // Never overwrite an existing impersonation context: RevertToSelf would lose it.
    // SAFETY: the current thread pseudo-handle is always valid.
    let thread = unsafe { GetCurrentThread() };
    let mut token = null_mut();
    // SAFETY: token is valid output storage.
    if unsafe { OpenThreadToken(thread, TOKEN_QUERY, 1, &mut token) } != 0 {
        drop(owned_handle(token)?);
        return Err(account_rejected());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() != Some(ERROR_NO_TOKEN as i32) {
        return Err(error);
    }

    // SAFETY: the server instance has just completed a read from its connected peer.
    check(unsafe { ImpersonateNamedPipeClient(stream.as_raw_handle()) })?;
    let restore = RestoreThread;
    // SAFETY: OpenAsSelf allows querying the identification-level token; no other work runs here.
    let result = check(unsafe { OpenThreadToken(thread, TOKEN_QUERY, 1, &mut token) })
        .and_then(|()| owned_handle(token));
    drop(restore);
    token_account(result?)
}

struct RestoreThread;

impl Drop for RestoreThread {
    fn drop(&mut self) {
        // SAFETY: this synchronous guard runs on the impersonating thread, including during unwind.
        if unsafe { RevertToSelf() } == 0 {
            // Continuing would run the runtime under a foreign thread identity.
            std::process::abort();
        }
    }
}

fn token_account(token: OwnedHandle) -> io::Result<String> {
    let mut size = 0;
    // SAFETY: a null buffer with zero length requests the required size only.
    if unsafe { GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut size) }
        != 0
    {
        return Err(account_rejected());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
        return Err(error);
    }
    if (size as usize) < size_of::<TOKEN_USER>() {
        return Err(account_rejected());
    }
    let mut data = vec![0_usize; (size as usize).div_ceil(size_of::<usize>())];
    // SAFETY: the buffer is aligned for TOKEN_USER and has at least size writable bytes.
    check(unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            data.as_mut_ptr().cast(),
            size,
            &mut size,
        )
    })?;
    // SAFETY: GetTokenInformation initialized TOKEN_USER; its SID lives in data.
    sid_text(unsafe { (*data.as_ptr().cast::<TOKEN_USER>()).User.Sid })
}

fn sid_text(sid: PSID) -> io::Result<String> {
    let mut text = null_mut();
    // SAFETY: callers pass a live Windows-supplied SID; text is a writable out-pointer.
    check(unsafe { ConvertSidToStringSidW(sid, &mut text) })?;
    let _allocation = LocalAllocation(text.cast());
    let mut len = 0;
    // SAFETY: ConvertSidToStringSidW supplied a NUL-terminated UTF-16 allocation.
    unsafe {
        while *text.add(len) != 0 {
            len += 1;
        }
        String::from_utf16(std::slice::from_raw_parts(text, len)).map_err(io::Error::other)
    }
}

fn owned_handle(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: only successful OpenProcess/Open*Token results are passed here.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

struct LocalAllocation(*mut c_void);

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        // SAFETY: the pointer is null or the unique LocalFree allocation returned by a Win32 call.
        unsafe { LocalFree(self.0) };
    }
}

fn check(result: i32) -> io::Result<()> {
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn account_rejected() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "IPC peer account unverified",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_ipc::HELLO_TIMEOUT;
    use std::path::PathBuf;
    use windows_sys::Win32::Security::{
        AclSizeInformation, GetAce, GetAclInformation, GetSecurityDescriptorControl,
        ACCESS_ALLOWED_ACE, ACL, ACL_SIZE_INFORMATION, DACL_SECURITY_INFORMATION,
        SE_DACL_PROTECTED,
    };
    use windows_sys::Win32::Storage::FileSystem::SECURITY_ANONYMOUS;

    fn test_endpoint(root: &tempfile::TempDir) -> PathBuf {
        let nonce = root.path().file_name().unwrap().to_str().unwrap();
        PathBuf::from(format!(
            r"\\.\pipe\aleph-s1b-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn endpoint_does_not_accept_remote_or_other_namespaces() {
        assert!(validate_endpoint(Path::new(r"\\.\pipe\gateway-fixture")).is_ok());
        for name in [
            r"\\remote\pipe\gateway",
            r"\\localhost\pipe\gateway",
            r"\\?\pipe\gateway",
            r"\\.\mailslot\gateway",
            r"\\.\pipe\",
            r"\\.\pipe\nested\gateway",
            r"\\.\pipe\gateway ",
            r"C:\gateway.sock",
        ] {
            assert!(validate_endpoint(Path::new(name)).is_err(), "{name}");
        }
    }

    fn assert_single_account_acl(server: &NamedPipeServer, expected: &PipeAccount) {
        let mut dacl: *mut ACL = null_mut();
        let mut owner = null_mut();
        let mut descriptor = LocalAllocation(null_mut());
        // SAFETY: all selected outputs are writable and the server handle is live.
        let rc = unsafe {
            GetSecurityInfo(
                server.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | OWNER_SECURITY_INFORMATION,
                &mut owner,
                null_mut(),
                &mut dacl,
                null_mut(),
                &mut descriptor.0,
            )
        };
        assert_eq!(rc, 0);
        assert!(!dacl.is_null());
        assert!(!owner.is_null());
        assert!(sid_text(owner).unwrap() == expected.0);
        let mut info = ACL_SIZE_INFORMATION::default();
        let mut ace = null_mut();
        let mut control = 0;
        let mut revision = 0;
        // SAFETY: dacl and its ACE storage live inside descriptor; destinations are writable.
        unsafe {
            check(GetSecurityDescriptorControl(
                descriptor.0,
                &mut control,
                &mut revision,
            ))
            .unwrap();
            assert_ne!(control & SE_DACL_PROTECTED, 0);
            check(GetAclInformation(
                dacl,
                (&raw mut info).cast(),
                size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            ))
            .unwrap();
            assert_eq!(info.AceCount, 1);
            check(GetAce(dacl, 0, &mut ace)).unwrap();
            assert_eq!((*ace.cast::<ACCESS_ALLOWED_ACE>()).Header.AceType, 0);
            let sid = (&raw mut (*ace.cast::<ACCESS_ALLOWED_ACE>()).SidStart).cast();
            assert!(sid_text(sid).unwrap() == expected.0);
        }
    }

    fn assert_no_impersonation() {
        let mut token = null_mut();
        // SAFETY: current-thread pseudo-handle and token out-pointer are valid.
        let success = unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut token) };
        if success != 0 {
            drop(owned_handle(token).unwrap());
            panic!("authentication left an impersonation token on the thread");
        }
        assert_eq!(
            io::Error::last_os_error().raw_os_error(),
            Some(ERROR_NO_TOKEN as i32)
        );
    }

    #[tokio::test]
    async fn anonymous_peer_is_rejected_and_does_not_leave_a_thread_token() {
        let root = tempfile::tempdir().unwrap();
        let name = test_endpoint(&root);
        let account = PipeAccount::current().unwrap();
        let mut server = account.create(&name, true, 8).unwrap();
        let mut client = ClientOptions::new()
            .security_qos_flags(SECURITY_ANONYMOUS)
            .open(&name)
            .unwrap();
        tokio::time::timeout(HELLO_TIMEOUT, async {
            server.connect().await.unwrap();
            client.write_u8(AUTH_BYTE).await.unwrap();
            assert!(account.authenticate_client(&mut server).await.is_err());
            assert_no_impersonation();
            server.disconnect().unwrap();
            assert!(client.read_u8().await.is_err());
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn client_rejects_a_connected_pipe_with_a_different_expected_owner() {
        let root = tempfile::tempdir().unwrap();
        let name = test_endpoint(&root);
        let account = PipeAccount::current().unwrap();
        let server = account.create(&name, true, 8).unwrap();
        let client = ClientOptions::new().open(&name).unwrap();
        require_server_account(&client, &account).unwrap();
        let wrong_owner = PipeAccount("S-1-0-0".to_owned());
        assert_eq!(
            require_server_account(&client, &wrong_owner)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        drop(server);
    }

    #[tokio::test]
    async fn owned_pipe_authenticates_and_refuses_a_second_first_instance() {
        let root = tempfile::tempdir().unwrap();
        let name = test_endpoint(&root);
        let account = PipeAccount::current().unwrap();
        let mut server = account.create(&name, true, 8).unwrap();
        assert_single_account_acl(&server, &account);
        let err = account.create(&name, true, 8).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        tokio::time::timeout(HELLO_TIMEOUT, async {
            let (client, ()) = tokio::join!(connect(&name), async {
                server.connect().await.unwrap();
                account.authenticate_client(&mut server).await.unwrap();
                assert_no_impersonation();
            });
            let _client = client.unwrap();
        })
        .await
        .unwrap();
    }
}
