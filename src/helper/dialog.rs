use super::pin_field::{self, PasswordFieldKind};
use crate::common::constants::OK_BUTTON_AUTOMATION_ID;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Accessibility::{
    IUIAutomation, IUIAutomationElement, IUIAutomationInvokePattern, IUIAutomationValuePattern,
    TreeScope_Descendants, UIA_ButtonControlTypeId, UIA_CONTROLTYPE_ID, UIA_InvokePatternId,
    UIA_ValuePatternId,
};
use windows::core::{BSTR, Interface, PWSTR, Result};
use zeroize::Zeroize;

/// Result of scanning a credential dialog's UI tree in a single pass.
pub struct DialogScanResult {
    pub has_password_field: bool,
    pub hello_pin_field: Option<IUIAutomationElement>,
    pub ok_button: Option<IUIAutomationElement>,
}

/// Scan all descendants of `dialog` in one FindAll pass.
/// Only expose an unambiguous Windows Hello PIN field for automatic entry.
pub fn scan_dialog(
    automation: &IUIAutomation,
    dialog: &IUIAutomationElement,
) -> Result<DialogScanResult> {
    let mut hello_pin_field = None;
    let mut password_fields = 0;
    let mut ok_button: Option<IUIAutomationElement> = None;

    unsafe {
        let condition = automation.CreateTrueCondition()?;
        let all_elements = dialog.FindAll(TreeScope_Descendants, &condition)?;
        let count = all_elements.Length()?;

        for i in 0..count {
            let el = all_elements.GetElement(i)?;
            let auto_id = el.CurrentAutomationId()?.to_string();
            let is_password = el.CurrentIsPassword()?.as_bool();

            if is_password || auto_id.starts_with("PasswordField_") {
                password_fields += 1;
                let label = el.CurrentName()?.to_string();
                if pin_field::classify(&auto_id, is_password, &label)
                    == PasswordFieldKind::WindowsHello
                {
                    hello_pin_field = Some(el.clone());
                }
            }

            if auto_id == OK_BUTTON_AUTOMATION_ID {
                let ctrl_type = el.CurrentControlType().unwrap_or(UIA_CONTROLTYPE_ID(0));
                if ctrl_type == UIA_ButtonControlTypeId {
                    ok_button = Some(el);
                }
            }
        }
    }

    // PIN changes, enrollment, and mixed credential forms require manual input.
    if password_fields != 1 {
        hello_pin_field = None;
    }

    Ok(DialogScanResult {
        has_password_field: password_fields > 0,
        hello_pin_field,
        ok_button,
    })
}

pub fn is_ready(element: &IUIAutomationElement) -> Result<bool> {
    unsafe { Ok(element.CurrentIsEnabled()?.as_bool() && !element.CurrentIsOffscreen()?.as_bool()) }
}

/// A matching window class alone can be spoofed by an unrelated application.
pub fn is_windows_hello_pin(
    dialog: &IUIAutomationElement,
    field: &IUIAutomationElement,
) -> Result<bool> {
    unsafe {
        let pid = dialog.CurrentProcessId()?;
        // Recheck the live label before both filling and submitting: switching
        // authentication methods can reuse the same dialog and field IDs.
        if pin_field::classify(
            &field.CurrentAutomationId()?.to_string(),
            field.CurrentIsPassword()?.as_bool(),
            &field.CurrentName()?.to_string(),
        ) != PasswordFieldKind::WindowsHello
            || field.CurrentProcessId()? != pid
        {
            return Ok(false);
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid as u32)?;
        let mut image = vec![0u16; 32768];
        let mut len = image.len() as u32;
        let result = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(image.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(process);
        result?;
        let mut system = vec![0u16; 32768];
        let system_len = GetSystemDirectoryW(Some(&mut system)) as usize;
        if system_len == 0 || system_len >= system.len() {
            return Ok(false);
        }
        Ok(is_broker_path(
            &String::from_utf16_lossy(&image[..len as usize]),
            &String::from_utf16_lossy(&system[..system_len]),
        ))
    }
}

fn is_broker_path(image: &str, system: &str) -> bool {
    image.eq_ignore_ascii_case(&format!("{system}\\CredentialUIBroker.exe"))
}

/// Set the specific PIN field directly; no keystrokes are sent to other windows.
pub fn fill_pin(field: &IUIAutomationElement, pin: &[u16]) -> Result<()> {
    unsafe {
        let pattern = field.GetCurrentPattern(UIA_ValuePatternId)?;
        let value: IUIAutomationValuePattern = pattern.cast()?;
        let text = BSTR::from_wide(pin);
        let result = value.SetValue(&text);
        // Take ownership of the allocation before mutating it, then restore its
        // owner so the erased temporary is freed with the correct allocator.
        let len = text.len();
        let raw = text.into_raw();
        if len > 0 {
            std::slice::from_raw_parts_mut(raw.cast_mut(), len).zeroize();
        }
        drop(BSTR::from_raw(raw));
        result
    }
}

/// Click a button via InvokePattern.
pub fn invoke_button(button: &IUIAutomationElement) -> Result<()> {
    unsafe {
        let pattern = button.GetCurrentPattern(UIA_InvokePatternId)?;
        let invoke: IUIAutomationInvokePattern = pattern.cast()?;
        invoke.Invoke()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_the_system_credential_broker_not_just_its_file_name() {
        let system = r"C:\Windows\System32";
        assert!(is_broker_path(
            r"c:\WINDOWS\system32\CredentialUIBroker.exe",
            system
        ));
        assert!(!is_broker_path(
            r"C:\Users\Public\CredentialUIBroker.exe",
            system
        ));
        assert!(!is_broker_path(r"C:\Windows\System32\notepad.exe", system));
        assert!(!is_broker_path(
            r"C:\Windows\System32\subdir\CredentialUIBroker.exe",
            system
        ));
    }
}
