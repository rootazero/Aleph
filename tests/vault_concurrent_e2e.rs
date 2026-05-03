//! Spec C Task 24 (part 2): two threads racing on `VaultIo::write`
//! must serialise via the fcntl exclusive lock. The final on-disk
//! payload must be ONE of the two writers' bytes — never a torn
//! mix of both.

use std::sync::Arc;
use std::thread;

use alephcore::utils::vault_io::VaultIo;

#[test]
fn two_threads_writing_vault_serialise_cleanly() {
    let dir = tempfile::tempdir().expect("tempdir");
    let io = Arc::new(VaultIo::new(dir.path()));

    let mut handles = vec![];
    for tag in 0..2u8 {
        let io = io.clone();
        handles.push(thread::spawn(move || {
            let payload = vec![tag; 1024];
            io.write(&payload).expect("vault write");
        }));
    }
    for h in handles {
        h.join().expect("thread join");
    }

    let final_bytes = io
        .read()
        .expect("vault read")
        .expect("vault must be populated");
    assert_eq!(final_bytes.len(), 1024, "size unchanged");
    let head = final_bytes[0];
    assert!(
        final_bytes.iter().all(|&b| b == head),
        "non-uniform vault bytes — torn write detected (head={head}, sample={:?})",
        &final_bytes[..16.min(final_bytes.len())]
    );
}
