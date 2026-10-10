use std::sync::OnceLock;

use windows::Win32::Foundation::{FreeLibrary, HINSTANCE};
use windows::Win32::System::LibraryLoader::{
    LOAD_LIBRARY_AS_DATAFILE, LOAD_LIBRARY_AS_IMAGE_RESOURCE, LOAD_LIBRARY_SEARCH_SYSTEM32,
    LoadLibraryExW,
};
use windows::Win32::UI::WindowsAndMessaging::LoadStringW;
use windows::core::{PCWSTR, PWSTR, w};

use crate::common::constants::PASSWORD_FIELD_AUTOMATION_ID;

#[derive(Debug, PartialEq)]
pub enum PasswordFieldKind {
    NotPassword,
    WindowsHello,
    Other,
}

struct PinLabels {
    hello: Vec<String>,
    security_key: Vec<String>,
}

impl PinLabels {
    fn load() -> Option<Self> {
        // These are field-label resources, not dialog titles or button names.
        // Resolve them from Windows so matching follows the user's UI language.
        // Ngc: current PIN / work PIN. FIDO: current / new / confirm key PIN.
        Some(Self {
            hello: load_labels(w!("NgcCredProv.dll"), &[105, 106])?,
            security_key: load_labels(w!("FidoCredProv.dll"), &[205, 208, 209])?,
        })
    }

    fn allows(&self, name: &str) -> bool {
        let name = name.trim();
        !name.is_empty()
            && self.hello.iter().any(|label| label == name)
            && !self.security_key.iter().any(|label| label == name)
    }
}

fn load_labels(module: PCWSTR, ids: &[u32]) -> Option<Vec<String>> {
    unsafe {
        let library = LoadLibraryExW(
            module,
            None,
            LOAD_LIBRARY_AS_DATAFILE
                | LOAD_LIBRARY_AS_IMAGE_RESOURCE
                | LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
        .ok()?;
        let labels = ids
            .iter()
            .map(|id| {
                let mut buffer = [0u16; 512];
                let len = LoadStringW(
                    Some(HINSTANCE(library.0)),
                    *id,
                    PWSTR(buffer.as_mut_ptr()),
                    buffer.len() as i32,
                );
                if len <= 0 || len as usize >= buffer.len() - 1 {
                    return None;
                }
                let label = String::from_utf16(&buffer[..len as usize]).ok()?;
                (!label.trim().is_empty()).then(|| label.trim().to_owned())
            })
            .collect();
        let _ = FreeLibrary(library);
        labels
    }
}

pub fn classify(id: &str, is_password: bool, name: &str) -> PasswordFieldKind {
    static LABELS: OnceLock<Option<PinLabels>> = OnceLock::new();
    classify_with_labels(
        id,
        is_password,
        name,
        LABELS.get_or_init(PinLabels::load).as_ref(),
    )
}

fn classify_with_labels(
    id: &str,
    is_password: bool,
    name: &str,
    labels: Option<&PinLabels>,
) -> PasswordFieldKind {
    if !is_password && !id.starts_with("PasswordField_") {
        return PasswordFieldKind::NotPassword;
    }
    if is_password
        && id == PASSWORD_FIELD_AUTOMATION_ID
        && labels.is_some_and(|labels| labels.allows(name))
    {
        PasswordFieldKind::WindowsHello
    } else {
        PasswordFieldKind::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chinese_labels() -> PinLabels {
        PinLabels {
            hello: vec!["PIN".into(), "工作 PIN".into()],
            security_key: vec![
                "安全密钥 PIN".into(),
                "新建安全密钥 PIN".into(),
                "确认安全密钥 PIN".into(),
            ],
        }
    }

    #[test]
    fn security_key_in_the_reported_screenshot_is_never_a_hello_pin() {
        let labels = chinese_labels();
        for name in [
            "安全密钥 PIN",
            "Security key PIN",
            "PIN for security key",
            "密码",
            "新 PIN",
            "",
        ] {
            assert_eq!(
                classify_with_labels("PasswordField_4", true, name, Some(&labels)),
                PasswordFieldKind::Other
            );
        }
        assert_eq!(
            classify_with_labels("PasswordField_4", true, "PIN", Some(&labels)),
            PasswordFieldKind::WindowsHello
        );
    }

    #[test]
    fn unrelated_password_fields_block_face_confirmation() {
        for (id, protected) in [
            ("PasswordField_0", true),
            ("PasswordField_2", false),
            ("CustomPin", true),
        ] {
            assert_eq!(
                classify_with_labels(id, protected, "PIN", Some(&chinese_labels())),
                PasswordFieldKind::Other
            );
        }
        assert_eq!(
            classify_with_labels("OkButton", false, "OK", None),
            PasswordFieldKind::NotPassword
        );
    }

    #[test]
    fn unknown_or_ambiguous_provider_labels_fail_closed() {
        assert_eq!(
            classify_with_labels("PasswordField_4", true, "PIN", None),
            PasswordFieldKind::Other
        );
        let mut labels = chinese_labels();
        labels.security_key.push("PIN".into());
        assert_eq!(
            classify_with_labels("PasswordField_4", true, "PIN", Some(&labels)),
            PasswordFieldKind::Other
        );
        assert_eq!(
            classify_with_labels("PasswordField_4", false, "PIN", Some(&chinese_labels())),
            PasswordFieldKind::Other
        );
    }

    #[test]
    fn installed_windows_provider_resources_distinguish_pin_types() {
        let Some(labels) = PinLabels::load() else {
            // Some Windows Server/CI images do not install these providers.
            assert_eq!(
                classify("PasswordField_4", true, "PIN"),
                PasswordFieldKind::Other
            );
            return;
        };
        for name in &labels.hello {
            assert!(labels.allows(name));
        }
        for name in &labels.security_key {
            assert!(!labels.allows(name));
        }
    }
}
