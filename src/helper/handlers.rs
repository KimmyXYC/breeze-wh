use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use windows::Win32::System::Com::SAFEARRAY;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    IUIAutomation, IUIAutomationElement, IUIAutomationFocusChangedEventHandler,
    IUIAutomationFocusChangedEventHandler_Impl, IUIAutomationPropertyChangedEventHandler,
    IUIAutomationPropertyChangedEventHandler_Impl, IUIAutomationStructureChangedEventHandler,
    IUIAutomationStructureChangedEventHandler_Impl, StructureChangeType, TreeScope_Subtree,
    UIA_E_ELEMENTNOTAVAILABLE, UIA_IsEnabledPropertyId, UIA_IsOffscreenPropertyId, UIA_PROPERTY_ID,
};
use windows::core::{Interface, Result, implement};
use windows_core::Ref;

use crate::common::constants::CREDENTIAL_DIALOG_CLASS;
use crate::common::pin;

use super::dialog;

/// Tracks a registered StructureChangedEventHandler for cleanup.
struct RegisteredHandler {
    handler: IUIAutomationStructureChangedEventHandler,
    property_handler: IUIAutomationPropertyChangedEventHandler,
    dialog: IUIAutomationElement,
    runtime_id: Vec<i32>,
}

#[derive(Clone, Copy)]
enum PinStage {
    Attempted,
    Filled,
}

struct PinAttempt {
    dialog: IUIAutomationElement,
    stage: PinStage,
}

#[derive(Debug, PartialEq)]
enum PinAction {
    Fill,
    Submit,
    Wait,
    Done,
}

fn pin_action(stage: Option<PinStage>, field_ready: Option<bool>, button_ready: bool) -> PinAction {
    match (stage, field_ready) {
        (Some(PinStage::Attempted), _) | (_, None) => PinAction::Done,
        (Some(PinStage::Filled), Some(_)) if button_ready => PinAction::Submit,
        (None, Some(true)) => PinAction::Fill,
        _ => PinAction::Wait,
    }
}

/// Shared state between FocusHandler and StructureHandler.
///
/// # Safety
/// All COM pointers are accessed exclusively from the STA message pump thread.
/// The `Mutex` fields are used for interior mutability, not cross-thread access.
unsafe impl Send for SharedState {}
unsafe impl Sync for SharedState {}

pub struct SharedState {
    pub automation: IUIAutomation,
    pub debounce_ms: u64,
    active_handler: Mutex<Option<RegisteredHandler>>,
    last_click: Mutex<Option<(Vec<i32>, Instant)>>,
    pin_attempts: Mutex<HashMap<Vec<i32>, PinAttempt>>,
    handling: AtomicBool,
}

impl SharedState {
    pub fn new(automation: IUIAutomation, debounce_ms: u64) -> Self {
        Self {
            automation,
            debounce_ms,
            active_handler: Mutex::new(None),
            last_click: Mutex::new(None),
            pin_attempts: Mutex::new(HashMap::new()),
            handling: AtomicBool::new(false),
        }
    }

    /// Remove the currently registered StructureChangedEventHandler, if any.
    fn remove_active_handler(&self) {
        let registered = self.active_handler.lock().unwrap().take();
        if let Some(reg) = registered {
            unsafe {
                let _ = self
                    .automation
                    .RemoveStructureChangedEventHandler(&reg.dialog, &reg.handler);
                let _ = self
                    .automation
                    .RemovePropertyChangedEventHandler(&reg.dialog, &reg.property_handler);
            }
            tracing::debug!("Structure change handler removed");
        }
    }

    /// Register a new StructureChangedEventHandler on the given dialog.
    fn register_structure_handler(&self, dialog: &IUIAutomationElement, shared: Arc<SharedState>) {
        let runtime_id = get_runtime_id(dialog);
        if self
            .active_handler
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|reg| !runtime_id.is_empty() && reg.runtime_id == runtime_id)
        {
            return;
        }
        // Remove any previous handler first
        self.remove_active_handler();

        let handler_iface: IUIAutomationStructureChangedEventHandler = StructureHandler {
            dialog: dialog.clone(),
            shared,
        }
        .into();
        let property_handler: IUIAutomationPropertyChangedEventHandler = handler_iface
            .cast()
            .expect("Handler implements both event interfaces");

        unsafe {
            match self.automation.AddStructureChangedEventHandler(
                dialog,
                TreeScope_Subtree,
                None,
                &handler_iface,
            ) {
                Ok(()) => {
                    if let Err(e) = self.automation.AddPropertyChangedEventHandlerNativeArray(
                        dialog,
                        TreeScope_Subtree,
                        None,
                        &property_handler,
                        &[UIA_IsEnabledPropertyId, UIA_IsOffscreenPropertyId],
                    ) {
                        tracing::warn!("Failed to register property handler: {}", e);
                    }
                    let mut guard = self.active_handler.lock().unwrap();
                    *guard = Some(RegisteredHandler {
                        handler: handler_iface,
                        property_handler,
                        dialog: dialog.clone(),
                        runtime_id,
                    });
                    tracing::debug!("Structure change handler registered");
                }
                Err(e) => {
                    tracing::warn!("Failed to register structure handler: {}", e);
                }
            }
        }
    }

    /// Check if this dialog was recently clicked (debounce).
    fn is_debounced(&self, runtime_id: &[i32]) -> bool {
        let guard = self.last_click.lock().unwrap();
        if let Some((ref id, ref instant)) = *guard
            && id == runtime_id
            && instant.elapsed().as_millis() < self.debounce_ms as u128
        {
            return true;
        }
        false
    }

    /// Record that we clicked a dialog.
    fn record_click(&self, runtime_id: Vec<i32>) {
        let mut guard = self.last_click.lock().unwrap();
        *guard = Some((runtime_id, Instant::now()));
    }

    fn pin_stage(&self, runtime_id: &[i32]) -> Option<PinStage> {
        let previous = self
            .pin_attempts
            .lock()
            .unwrap()
            .get(runtime_id)
            .map(|attempt| (attempt.dialog.clone(), attempt.stage));
        let (dialog, stage) = previous?;
        // UIA runtime IDs may be reused after the original element is destroyed.
        if is_closed(&dialog) {
            self.pin_attempts.lock().unwrap().remove(runtime_id);
            return None;
        }
        Some(stage)
    }

    fn set_pin_stage(&self, runtime_id: &[i32], stage: PinStage) {
        if let Some(attempt) = self.pin_attempts.lock().unwrap().get_mut(runtime_id) {
            attempt.stage = stage;
        }
    }

    fn record_pin_attempt(&self, runtime_id: Vec<i32>, dialog: &IUIAutomationElement) {
        // Do not hold a mutex over a COM call: providers may dispatch nested events.
        let previous: Vec<_> = self
            .pin_attempts
            .lock()
            .unwrap()
            .iter()
            .map(|(id, attempt)| (id.clone(), attempt.dialog.clone()))
            .collect();
        let closed: Vec<_> = previous
            .into_iter()
            .filter_map(|(id, element)| is_closed(&element).then_some(id))
            .collect();
        let mut attempts = self.pin_attempts.lock().unwrap();
        for id in closed {
            attempts.remove(&id);
        }
        attempts.insert(
            runtime_id,
            PinAttempt {
                dialog: dialog.clone(),
                stage: PinStage::Attempted,
            },
        );
    }
}

fn is_closed(element: &IUIAutomationElement) -> bool {
    unsafe { element.CurrentProcessId() }
        .is_err_and(|error| error.code().0 as u32 == UIA_E_ELEMENTNOTAVAILABLE)
}

/// Get the runtime ID of a UIA element (stable identifier for debounce).
fn get_runtime_id(element: &IUIAutomationElement) -> Vec<i32> {
    unsafe {
        element
            .GetRuntimeId()
            .ok()
            .and_then(|sa| {
                if sa.is_null() {
                    return None;
                }
                let sa_ref = &*sa;
                let len = sa_ref.rgsabound[0].cElements as usize;
                let data = sa_ref.pvData as *const i32;
                let id = if data.is_null() || len == 0 {
                    None
                } else {
                    Some(std::slice::from_raw_parts(data, len).to_vec())
                };
                let _ = windows::Win32::System::Ole::SafeArrayDestroy(sa);
                id
            })
            .unwrap_or_default()
    }
}

/// Check if an element is a credential dialog by class name.
fn is_credential_dialog(element: &IUIAutomationElement) -> bool {
    unsafe {
        let class = element.CurrentClassName().unwrap_or_default().to_string();
        class == CREDENTIAL_DIALOG_CLASS
    }
}

/// Try to handle a detected credential dialog: scan and click if appropriate.
/// Returns true when no further watching is needed.
fn try_handle_dialog(shared: &Arc<SharedState>, dialog: &IUIAutomationElement) -> Result<bool> {
    // SetValue/Invoke can synchronously cause more UIA callbacks.
    if shared.handling.swap(true, Ordering::SeqCst) {
        return Ok(true);
    }
    struct HandlingGuard<'a>(&'a AtomicBool);
    impl Drop for HandlingGuard<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }
    let _guard = HandlingGuard(&shared.handling);

    let mut scan = dialog::scan_dialog(&shared.automation, dialog)?;
    let rid = get_runtime_id(dialog);
    if scan.password_field.is_some() || shared.pin_stage(&rid).is_some() {
        // Without an identity we cannot prevent repeated PIN attempts.
        if rid.is_empty() {
            shared.remove_active_handler();
            return Ok(true);
        }
        loop {
            let field_ready = scan
                .password_field
                .as_ref()
                .map(dialog::is_ready)
                .transpose()?;
            let button_ready = scan
                .ok_button
                .as_ref()
                .map(dialog::is_ready)
                .transpose()?
                .unwrap_or(false);
            match pin_action(shared.pin_stage(&rid), field_ready, button_ready) {
                PinAction::Fill => {
                    if !dialog::is_pin_host(dialog, scan.password_field.as_ref().unwrap())? {
                        tracing::debug!(
                            "PIN field is not hosted by the system credential broker; skipping"
                        );
                        shared.remove_active_handler();
                        return Ok(true);
                    }
                    let Some(pin) = pin::load()? else {
                        tracing::debug!("PIN mode detected, no saved PIN; skipping");
                        return Ok(false);
                    };
                    // Mark the attempt before SetValue, including failed writes.
                    shared.record_pin_attempt(rid.clone(), dialog);
                    if let Err(e) = dialog::fill_pin(scan.password_field.as_ref().unwrap(), &pin) {
                        // A provider's error text may contain its input. Log only the code.
                        tracing::warn!(code = ?e.code(), "Could not fill PIN; use manual entry");
                        shared.remove_active_handler();
                        return Ok(true);
                    }
                    shared.set_pin_stage(&rid, PinStage::Filled);
                    tracing::info!("PIN filled");
                    scan = dialog::scan_dialog(&shared.automation, dialog)?;
                }
                PinAction::Submit => {
                    // Consume the attempt before invoking: an error may still mean
                    // the provider processed it, so never automatically retry.
                    shared.set_pin_stage(&rid, PinStage::Attempted);
                    shared.record_click(rid);
                    shared.remove_active_handler();
                    dialog::invoke_button(scan.ok_button.as_ref().unwrap())?;
                    tracing::info!("PIN confirmation submitted");
                    return Ok(true);
                }
                PinAction::Wait => return Ok(false),
                PinAction::Done => {
                    shared.set_pin_stage(&rid, PinStage::Attempted);
                    shared.remove_active_handler();
                    return Ok(true);
                }
            }
        }
    }

    if let Some(ref button) = scan.ok_button {
        if shared.is_debounced(&rid) || !dialog::is_ready(button)? {
            return Ok(false);
        }
        let name = unsafe { button.CurrentName().unwrap_or_default() };
        tracing::info!(name = %name, "Face recognition confirmed, clicking OkButton");

        dialog::invoke_button(button)?;

        shared.record_click(rid);
        shared.remove_active_handler();

        tracing::info!("Auto-click successful");
        return Ok(true);
    }

    Ok(false)
}

// ── StructureChangedHandler ──

#[implement(
    IUIAutomationStructureChangedEventHandler,
    IUIAutomationPropertyChangedEventHandler
)]
struct StructureHandler {
    dialog: IUIAutomationElement,
    shared: Arc<SharedState>,
}

impl IUIAutomationPropertyChangedEventHandler_Impl for StructureHandler_Impl {
    fn HandlePropertyChangedEvent(
        &self,
        _sender: Ref<'_, IUIAutomationElement>,
        _property_id: UIA_PROPERTY_ID,
        _new_value: &VARIANT,
    ) -> Result<()> {
        if let Err(e) = try_handle_dialog(&self.shared, &self.dialog) {
            tracing::debug!("Property event error (dialog may have closed): {}", e);
            self.shared.remove_active_handler();
        }
        Ok(())
    }
}

impl IUIAutomationStructureChangedEventHandler_Impl for StructureHandler_Impl {
    fn HandleStructureChangedEvent(
        &self,
        _sender: Ref<'_, IUIAutomationElement>,
        _change_type: StructureChangeType,
        _runtime_id: *const SAFEARRAY,
    ) -> Result<()> {
        match try_handle_dialog(&self.shared, &self.dialog) {
            Ok(true) => {} // success — handler will be removed by try_handle_dialog
            Ok(false) => {}
            Err(e) => {
                // Dialog may have closed — remove handler
                tracing::debug!("Structure event error (dialog may have closed): {}", e);
                self.shared.remove_active_handler();
            }
        }
        Ok(())
    }
}

// ── FocusChangedHandler ──

#[implement(IUIAutomationFocusChangedEventHandler)]
pub struct FocusHandler {
    pub shared: Arc<SharedState>,
}

impl IUIAutomationFocusChangedEventHandler_Impl for FocusHandler_Impl {
    fn HandleFocusChangedEvent(&self, sender: Ref<'_, IUIAutomationElement>) -> Result<()> {
        let Some(element) = sender.as_ref() else {
            return Ok(());
        };

        // Skip events from our own process
        unsafe {
            let pid = element.CurrentProcessId().unwrap_or(0) as u32;
            if pid == GetCurrentProcessId() {
                return Ok(());
            }
        }

        let shared = &self.shared;

        unsafe {
            let walker = shared.automation.ControlViewWalker()?;

            // Walk up ancestors to find a credential dialog
            let mut current = element.clone();
            for _ in 0..10 {
                if is_credential_dialog(&current) {
                    // Debounce: skip if same dialog was recently handled
                    let rid = get_runtime_id(&current);
                    if shared.is_debounced(&rid) {
                        tracing::trace!("Debounced — same dialog recently handled");
                        return Ok(());
                    }

                    tracing::info!("Credential dialog detected");

                    // Subscribe before scanning to catch readiness changes caused by SetValue.
                    shared.register_structure_handler(&current, Arc::clone(shared));
                    match try_handle_dialog(shared, &current) {
                        Ok(true) => {
                            // Clicked successfully
                        }
                        Ok(false) => {
                            tracing::debug!("Dialog not ready, watching for changes");
                        }
                        Err(e) => {
                            tracing::warn!("Error handling dialog: {}", e);
                        }
                    }
                    return Ok(());
                }

                match walker.GetParentElement(&current) {
                    Ok(parent) => current = parent,
                    Err(_) => break,
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_for_field_then_submits_only_after_button_is_ready() {
        assert_eq!(pin_action(None, Some(false), false), PinAction::Wait);
        assert_eq!(pin_action(None, Some(true), false), PinAction::Fill);
        assert_eq!(
            pin_action(Some(PinStage::Filled), Some(true), false),
            PinAction::Wait
        );
        assert_eq!(
            pin_action(Some(PinStage::Filled), Some(true), true),
            PinAction::Submit
        );
    }

    #[test]
    fn failed_or_submitted_attempts_are_never_repeated() {
        // Further focus, visibility, and enabled events must not refill or submit.
        for field in [None, Some(false), Some(true)] {
            for button in [false, true] {
                assert_eq!(
                    pin_action(Some(PinStage::Attempted), field, button),
                    PinAction::Done
                );
            }
        }
    }

    #[test]
    fn disappearing_pin_field_does_not_trigger_a_second_confirmation() {
        // A provider can auto-submit on input or switch to another auth method.
        assert_eq!(
            pin_action(Some(PinStage::Filled), None, true),
            PinAction::Done
        );
        assert_eq!(
            pin_action(Some(PinStage::Filled), None, false),
            PinAction::Done
        );
    }

    #[test]
    fn enabled_ok_button_never_submits_before_filling() {
        assert_eq!(pin_action(None, Some(true), true), PinAction::Fill);
        assert_eq!(pin_action(None, Some(false), true), PinAction::Wait);
    }
}
