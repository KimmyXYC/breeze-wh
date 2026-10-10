use windows::Win32::Foundation::{E_INVALIDARG, ERROR_NOT_FOUND};
use windows::Win32::Security::Credentials::{
    CRED_MAX_CREDENTIAL_BLOB_SIZE, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW,
    CredDeleteW, CredFree, CredReadW, CredWriteW,
};
use windows::core::{Error, PCWSTR, PWSTR, Result, w};
use zeroize::{Zeroize, Zeroizing};

// Credentials belong to the interactive user, including when the helper uses
// that user's linked elevated token. Never store PINs in the shared config.
const TARGET: windows::core::PCWSTR = w!("Breeze-WH/WindowsHelloPIN");

fn validate(pin: &[u16]) -> Result<()> {
    if pin.is_empty()
        || std::mem::size_of_val(pin) > CRED_MAX_CREDENTIAL_BLOB_SIZE as usize
        || pin.iter().any(|c| matches!(c, 0 | 10 | 13))
        || char::decode_utf16(pin.iter().copied()).any(|c| c.is_err())
    {
        return Err(Error::new(
            E_INVALIDARG,
            "PIN is empty, too long, or contains invalid characters",
        ));
    }
    Ok(())
}

pub fn save(pin: &str) -> Result<()> {
    save_to(TARGET, pin)
}

fn save_to(target: PCWSTR, pin: &str) -> Result<()> {
    let mut wide = Zeroizing::new(pin.encode_utf16().collect::<Vec<_>>());
    validate(&wide)?;
    let credential = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(target.as_ptr().cast_mut()),
        CredentialBlobSize: std::mem::size_of_val(wide.as_slice()) as u32,
        CredentialBlob: wide.as_mut_ptr().cast(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        ..Default::default()
    };
    unsafe { CredWriteW(&credential, 0) }
}

struct StoredCredential(*mut CREDENTIALW);

impl Drop for StoredCredential {
    fn drop(&mut self) {
        unsafe {
            let credential = &mut *self.0;
            if !credential.CredentialBlob.is_null() && credential.CredentialBlobSize > 0 {
                std::slice::from_raw_parts_mut(
                    credential.CredentialBlob,
                    credential.CredentialBlobSize as usize,
                )
                .zeroize();
            }
            CredFree(self.0.cast());
        }
    }
}

pub fn load() -> Result<Option<Zeroizing<Vec<u16>>>> {
    load_from(TARGET)
}

fn load_from(target: PCWSTR) -> Result<Option<Zeroizing<Vec<u16>>>> {
    let mut raw = std::ptr::null_mut();
    match unsafe { CredReadW(target, CRED_TYPE_GENERIC, None, &mut raw) } {
        Ok(()) => {}
        Err(e) if e.code() == ERROR_NOT_FOUND.to_hresult() => return Ok(None),
        Err(e) => return Err(e),
    }
    let stored = StoredCredential(raw);
    let credential = unsafe { &*stored.0 };
    let size = credential.CredentialBlobSize as usize;
    if size == 0 || size % 2 != 0 || credential.CredentialBlob.is_null() {
        return Err(Error::new(E_INVALIDARG, "Invalid saved PIN"));
    }
    let bytes = unsafe { std::slice::from_raw_parts(credential.CredentialBlob, size) };
    let pin = Zeroizing::new(
        bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect::<Vec<_>>(),
    );
    validate(&pin)?;
    Ok(Some(pin))
}

pub fn clear() -> Result<()> {
    clear_from(TARGET)
}

fn clear_from(target: PCWSTR) -> Result<()> {
    match unsafe { CredDeleteW(target, CRED_TYPE_GENERIC, None) } {
        Err(e) if e.code() == ERROR_NOT_FOUND.to_hresult() => Ok(()),
        result => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_leading_zeroes_and_alphanumeric_pins() {
        for pin in ["001234", "Hello-123!", "密码123"] {
            assert!(validate(&pin.encode_utf16().collect::<Vec<_>>()).is_ok());
        }
    }

    #[test]
    fn rejects_invalid_credentials_without_exposing_them() {
        for pin in [
            vec![],
            vec![0],
            vec![10],
            vec![13],
            vec![0xd800],
            vec![65; 1281],
        ] {
            assert!(validate(&pin).is_err());
        }
        let error = validate(&"secret\n".encode_utf16().collect::<Vec<_>>()).unwrap_err();
        assert!(!error.to_string().contains("secret"));
    }

    #[test]
    fn credential_manager_round_trip_and_delete() {
        // Use an isolated name; never read or modify the user's configured PIN.
        let name: Vec<u16> = format!("Breeze-WH/TestPIN/{}", std::process::id())
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let target = PCWSTR(name.as_ptr());
        struct Cleanup(PCWSTR);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = clear_from(self.0);
            }
        }
        let _cleanup = Cleanup(target);
        assert!(load_from(target).unwrap().is_none());
        for value in ["001234", "Letters-123!", "密码123"] {
            save_to(target, value).unwrap();
            assert_eq!(
                load_from(target).unwrap().unwrap().as_slice(),
                value.encode_utf16().collect::<Vec<_>>()
            );
        }
        assert!(save_to(target, "").is_err());
        assert!(load_from(target).unwrap().is_some());
        clear_from(target).unwrap();
        assert!(load_from(target).unwrap().is_none());
        clear_from(target).unwrap();
    }
}
