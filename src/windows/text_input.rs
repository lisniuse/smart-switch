use std::sync::mpsc::{self, Receiver, SyncSender};

use windows::Win32::System::Com::*;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::Input::KeyboardAndMouse::HKL;
use windows::Win32::UI::TextServices::*;
use windows::core::Interface;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    ES_READONLY, GWL_STYLE, GetClassNameW, GetForegroundWindow, GetWindowLongW,
};

pub(super) struct Apartment;

impl Apartment {
    pub(super) fn enter(model: COINIT) -> Option<Self> {
        unsafe { CoInitializeEx(None, model).ok().ok()? };
        Some(Self)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

pub(super) struct Ime {
    profiles: ITfInputProcessorProfiles,
    manager: ITfInputProcessorProfileMgr,
    doubao: Option<TF_INPUTPROCESSORPROFILE>,
}

impl Ime {
    pub(super) fn new() -> Option<Self> {
        let profiles: ITfInputProcessorProfiles = unsafe {
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER).ok()?
        };
        let manager = profiles.cast().ok()?;
        Some(Self {
            profiles,
            manager,
            doubao: None,
        })
    }

    pub(super) fn doubao_profile(&mut self) -> Option<TF_INPUTPROCESSORPROFILE> {
        if self.doubao.is_none() {
            self.doubao = self.find_doubao();
        }
        self.doubao
    }

    fn find_doubao(&self) -> Option<TF_INPUTPROCESSORPROFILE> {
        const SIMPLIFIED_CHINESE: u16 = 0x0804;
        let items = unsafe { self.manager.EnumProfiles(SIMPLIFIED_CHINESE).ok()? };
        let mut profile = TF_INPUTPROCESSORPROFILE::default();
        loop {
            let mut fetched = 0;
            unsafe {
                items
                    .Next(std::slice::from_mut(&mut profile), &mut fetched)
                    .ok()?
            };
            if fetched == 0 {
                return None;
            }
            if profile.dwProfileType != TF_PROFILETYPE_INPUTPROCESSOR
                || profile.dwFlags & TF_IPP_FLAG_ENABLED == 0
            {
                continue;
            }
            let name = unsafe {
                self.profiles.GetLanguageProfileDescription(
                    &profile.clsid,
                    profile.langid,
                    &profile.guidProfile,
                )
            };
            if name.is_ok_and(|name| name.to_string().contains("豆包输入法")) {
                return Some(profile);
            }
        }
    }

    pub(super) fn is_active(&self, profile: &TF_INPUTPROCESSORPROFILE) -> bool {
        let mut active = TF_INPUTPROCESSORPROFILE::default();
        unsafe {
            self.manager
                .GetActiveProfile(&GUID_TFCAT_TIP_KEYBOARD, &mut active)
        }
        .is_ok()
            && active.clsid == profile.clsid
            && active.guidProfile == profile.guidProfile
    }

    pub(super) fn activate(&self, profile: &TF_INPUTPROCESSORPROFILE) -> bool {
        unsafe {
            self.manager.ActivateProfile(
                TF_PROFILETYPE_INPUTPROCESSOR,
                profile.langid,
                &profile.clsid,
                &profile.guidProfile,
                HKL::default(),
                TF_IPPMF_FORSESSION | TF_IPPMF_DONTCARECURRENTINPUTLANGUAGE,
            )
        }
        .is_ok()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct EditFocus {
    // Window handles are opaque identifiers shared with the probe thread.
    foreground: usize,
    target: usize,
}

impl EditFocus {
    pub(super) fn new(foreground: HWND, target: HWND) -> Self {
        Self {
            foreground: foreground as usize,
            target: target as usize,
        }
    }

    fn is_current(self) -> bool {
        let foreground = unsafe { GetForegroundWindow() };
        foreground as usize == self.foreground
            && super::keyboard_target(foreground).0 as usize == self.target
    }

    fn native_edit(self) -> Option<FocusState> {
        let hwnd = self.target as HWND;
        let mut name = [0u16; 64];
        let length = unsafe { GetClassNameW(hwnd, name.as_mut_ptr(), name.len() as i32) };
        let name = String::from_utf16_lossy(&name[..length as usize]).to_ascii_lowercase();
        if name != "edit"
            && !name.starts_with("richedit")
            && !name.starts_with("windowsforms10.edit.")
        {
            return None;
        }
        let style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) };
        let writable = style & ES_READONLY == 0 && unsafe { IsWindowEnabled(hwnd) } != 0;
        Some(if writable {
            FocusState::Edit(EditId(0))
        } else {
            FocusState::Other
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct EditId(u64);

#[derive(Clone, Copy)]
pub(super) enum FocusState {
    Pending,
    Other,
    Edit(EditId),
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Sample {
    focus: EditFocus,
    generation: u64,
}

struct Observation {
    request: Sample,
    state: FocusState,
}

pub(super) struct FocusProbe {
    requests: SyncSender<Sample>,
    observations: Receiver<Observation>,
    pending: bool,
    last: Option<Observation>,
    focus: Option<EditFocus>,
    generation: u64,
}

impl FocusProbe {
    pub(super) fn start() -> Option<Self> {
        let (requests, work) = mpsc::sync_channel(1);
        let (report, observations) = mpsc::channel();
        // UI Automation can wait for another application. Keep the tray and fullscreen detection responsive.
        std::thread::Builder::new()
            .name("text-focus".into())
            .spawn(move || observe(work, report))
            .ok()?;
        Some(Self {
            requests,
            observations,
            pending: false,
            last: None,
            focus: None,
            generation: 0,
        })
    }

    pub(super) fn clear(&mut self) {
        if self.focus.take().is_some() {
            self.generation += 1;
            self.last = None;
        }
    }

    pub(super) fn inspect(&mut self, focus: EditFocus) -> FocusState {
        if let Some(state) = focus.native_edit() {
            self.clear();
            return state;
        }
        if self.focus != Some(focus) {
            self.clear();
            self.focus = Some(focus);
        }
        let request = Sample {
            focus,
            generation: self.generation,
        };
        if let Ok(observation) = self.observations.try_recv() {
            self.pending = false;
            if observation.request == request {
                self.last = Some(observation);
            }
        }
        if !self.pending {
            self.pending = self.requests.try_send(request).is_ok();
        }
        self.last
            .as_ref()
            .map_or(FocusState::Pending, |observation| observation.state)
    }
}

fn observe(work: Receiver<Sample>, report: mpsc::Sender<Observation>) {
    let Some(_apartment) = Apartment::enter(COINIT_MULTITHREADED) else {
        return;
    };
    let Ok(automation): windows::core::Result<IUIAutomation2> =
        (unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) })
    else {
        return;
    };
    unsafe {
        let _ = automation.SetConnectionTimeout(500);
        let _ = automation.SetTransactionTimeout(500);
    }
    let mut previous = None;
    let mut id = EditId(0);
    while let Ok(request) = work.recv() {
        let state = match editable_element(&automation, request.focus) {
            Ok(Some(current)) => {
                let same = previous.as_ref().is_some_and(|old: &IUIAutomationElement| {
                    unsafe { automation.CompareElements(old, &current) }
                        .is_ok_and(|equal| equal.as_bool())
                });
                if !same {
                    id.0 += 1;
                }
                previous = Some(current);
                FocusState::Edit(id)
            }
            Ok(None) => {
                previous = None;
                FocusState::Other
            }
            Err(_) => FocusState::Pending,
        };
        if report.send(Observation { request, state }).is_err() {
            break;
        }
    }
}

fn editable_element(
    automation: &IUIAutomation2,
    focus: EditFocus,
) -> windows::core::Result<Option<IUIAutomationElement>> {
    if !focus.is_current() {
        return Ok(None);
    }
    // Read accessibility metadata only. Never retrieve the field's text or value.
    let element = unsafe {
        let element = automation.GetFocusedElement()?;
        if !element.CurrentHasKeyboardFocus()?.as_bool() || !element.CurrentIsEnabled()?.as_bool() {
            return Ok(None);
        }
        let kind = element.CurrentControlType()?;
        if kind != UIA_EditControlTypeId && kind != UIA_DocumentControlTypeId {
            return Ok(None);
        }
        let writable = match element
            .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
        {
            Ok(value) => !value.CurrentIsReadOnly()?.as_bool(),
            Err(_) => {
                kind == UIA_EditControlTypeId
                    || element
                        .GetCurrentPatternAs::<IUIAutomationTextEditPattern>(UIA_TextEditPatternId)
                        .is_ok()
            }
        };
        writable.then_some(element)
    };
    Ok(element.filter(|_| focus.is_current()))
}
