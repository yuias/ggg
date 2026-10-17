//! OLE side of the drop target: reads an `IDataObject` and answers the drag
//! calls. All COM use of the GUI is in this file.

use super::{
    DropPayload, choose_effect, first_urls, has_url_extension, modifiers, single_url, url_file_candidates,
    urls_from_url_files, wide_until_nul,
};
use ggg::util::url_input::parse_url_text;
use std::{cell::Cell, ffi::OsString, fs, os::windows::ffi::OsStringExt as _, path::PathBuf, slice, sync::OnceLock};
use windows::{
    Win32::{
        Foundation::{HWND, POINTL},
        System::{
            Com::{DVASPECT_CONTENT, FORMATETC, IDataObject, STGMEDIUM, TYMED_HGLOBAL},
            DataExchange::RegisterClipboardFormatW,
            Memory::{GlobalLock, GlobalSize, GlobalUnlock},
            Ole::{
                CF_HDROP, CF_UNICODETEXT, DROPEFFECT, IDropTarget, IDropTarget_Impl, OleInitialize,
                OleUninitialize, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop,
            },
            SystemServices::MODIFIERKEYS_FLAGS,
        },
        UI::Shell::{DragQueryFileW, HDROP},
    },
    core::{Ref, Result, implement, w},
};

/// More files than this in one drop are not looked at; the registration limit
/// is far below it.
const MAX_DROPPED_FILES: u32 = 10_000;

/// Longest text read from a drop, in UTF-16 units (4 MiB of text).
const MAX_TEXT_UNITS: usize = 2 * 1024 * 1024;

/// Keeps OLE initialised on this thread while it lives. Drop it after every
/// window is gone.
pub struct OleGuard;

impl Drop for OleGuard {
    fn drop(&mut self) {
        // SAFETY: balances the successful `OleInitialize` that created the guard.
        unsafe { OleUninitialize() };
    }
}

/// Initialises OLE as a single-threaded apartment on the calling thread, which
/// drag and drop needs. Winit's own COM setup joins the same apartment.
/// `None` (logged) leaves the GUI without a drop target.
pub fn init_ole() -> Option<OleGuard> {
    // SAFETY: called once on the main thread before any window exists.
    match unsafe { OleInitialize(None) } {
        Ok(()) => Some(OleGuard),
        Err(e) => {
            tracing::warn!("OLE is unavailable, so drag and drop is off: {e}");
            None
        }
    }
}

/// Registration of a window as a drop target; revoked when dropped.
pub struct DropRegistration {
    hwnd: HWND,
}

impl Drop for DropRegistration {
    fn drop(&mut self) {
        // Winit also revokes when the window is destroyed. Whichever comes
        // second fails harmlessly.
        // SAFETY: plain call on the registering thread.
        let _ = unsafe { RevokeDragDrop(self.hwnd) };
    }
}

/// Makes the window `hwnd` accept drops and hands each one to `sink`. The
/// sink runs inside the drag source's call, so it must return quickly.
pub fn register_drop_target(
    hwnd: isize,
    sink: impl Fn(DropPayload) + 'static,
) -> Option<DropRegistration> {
    let hwnd = HWND(hwnd as *mut std::ffi::c_void);
    let target: IDropTarget = DropTarget::new(Box::new(sink)).into();
    // SAFETY: `hwnd` belongs to a live window of this thread, which is in an
    // apartment initialised by `init_ole`.
    match unsafe { RegisterDragDrop(hwnd, &target) } {
        Ok(()) => Some(DropRegistration { hwnd }),
        Err(e) => {
            tracing::warn!("Cannot register the drop target: {e}");
            None
        }
    }
}

/// Clipboard format id of `UniformResourceLocatorW`, which browsers use for
/// the URL of a dragged link or image.
fn url_format() -> u16 {
    static ID: OnceLock<u16> = OnceLock::new();
    *ID.get_or_init(|| {
        // SAFETY: the name is a valid NUL-terminated wide string. Registered
        // format ids fit in 16 bits (0xC000..=0xFFFF).
        unsafe { RegisterClipboardFormatW(w!("UniformResourceLocatorW")) as u16 }
    })
}

fn format(cf: u16) -> FORMATETC {
    FORMATETC {
        cfFormat: cf,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

fn has_format(data: &IDataObject, cf: u16) -> bool {
    // SAFETY: the FORMATETC lives for the call.
    unsafe { data.QueryGetData(&format(cf)) }.is_ok()
}

/// A global-memory medium released when it goes out of scope.
struct Medium(STGMEDIUM);

impl Medium {
    fn get(data: &IDataObject, cf: u16) -> Option<Self> {
        // SAFETY: the FORMATETC lives for the call.
        let medium = Self(unsafe { data.GetData(&format(cf)) }.ok()?);
        // A medium of another kind is released by `Drop` and not used.
        (medium.0.tymed == TYMED_HGLOBAL.0 as u32).then_some(medium)
    }
}

impl Drop for Medium {
    fn drop(&mut self) {
        // SAFETY: the medium came from `GetData` and is released exactly once.
        unsafe { ReleaseStgMedium(&mut self.0) };
    }
}

/// NUL-terminated UTF-16 text in the medium, bounded by the block size.
fn read_wide_text(data: &IDataObject, cf: u16) -> Option<String> {
    let medium = Medium::get(data, cf)?;
    // SAFETY: tymed was checked to be HGLOBAL; the block stays locked while
    // the slice over it is alive and is copied into a String before unlock.
    unsafe {
        let handle = medium.0.u.hGlobal;
        // Capped so a huge text drop is not converted in full: no URL list
        // that matters is anywhere near this size.
        let units = (GlobalSize(handle) / 2).min(MAX_TEXT_UNITS);
        let ptr = GlobalLock(handle) as *const u16;
        if ptr.is_null() {
            return None;
        }
        let text = wide_until_nul(slice::from_raw_parts(ptr, units));
        let _ = GlobalUnlock(handle);
        Some(text)
    }
}

/// Paths in a `CF_HDROP` medium.
fn read_dropped_paths(data: &IDataObject) -> Vec<PathBuf> {
    let Some(medium) = Medium::get(data, CF_HDROP.0) else {
        return Vec::new();
    };
    // SAFETY: tymed was checked to be HGLOBAL, and an HDROP is that handle.
    // `DragQueryFileW` locks the block itself.
    unsafe {
        let hdrop = HDROP(medium.0.u.hGlobal.0);
        let count = DragQueryFileW(hdrop, u32::MAX, None).min(MAX_DROPPED_FILES);
        (0..count)
            .filter_map(|index| {
                let len = DragQueryFileW(hdrop, index, None) as usize;
                if len == 0 {
                    return None;
                }
                let mut buf = vec![0u16; len + 1];
                let copied = DragQueryFileW(hdrop, index, Some(&mut buf)) as usize;
                Some(PathBuf::from(OsString::from_wide(&buf[..copied.min(len)])))
            })
            .collect()
    }
}

/// Dropped files that are `.url` shortcuts of a readable size, produced one
/// at a time. The extension is checked before the file is looked at, so a
/// large drop of other files costs no file-system access.
fn dropped_url_files(data: &IDataObject) -> impl Iterator<Item = PathBuf> {
    read_dropped_paths(data)
        .into_iter()
        .filter(|path| has_url_extension(path))
        .filter_map(|path| {
            let meta = fs::metadata(&path).ok().filter(|m| m.is_file())?;
            url_file_candidates(&[(path, meta.len())]).pop()
        })
}

/// Whether a drag of `data` is worth a drop. Text formats count by presence;
/// a file list only when it holds a `.url` file, so a `.txt` from Explorer
/// shows the no-drop cursor.
fn accepts(data: &IDataObject) -> bool {
    has_format(data, url_format())
        || has_format(data, CF_UNICODETEXT.0)
        || (has_format(data, CF_HDROP.0) && dropped_url_files(data).next().is_some())
}

/// URLs in `data`: `UniformResourceLocatorW`, else `CF_UNICODETEXT`, else
/// `.url` files. URL formats go first because Firefox also offers a temp-file
/// `CF_HDROP` when an image is dragged.
fn extract_urls(data: &IDataObject) -> Vec<String> {
    first_urls(&mut [
        &mut || {
            read_wide_text(data, url_format())
                .map(|text| single_url(&text))
                .unwrap_or_default()
        },
        &mut || {
            read_wide_text(data, CF_UNICODETEXT.0)
                .map(|text| parse_url_text(&text))
                .unwrap_or_default()
        },
        &mut || urls_from_url_files(&dropped_url_files(data).collect::<Vec<_>>()),
    ])
}

#[implement(IDropTarget)]
struct DropTarget {
    sink: Box<dyn Fn(DropPayload)>,
    /// Verdict of `DragEnter`; `DragOver` gets no data object to ask again.
    accepted: Cell<bool>,
}

impl DropTarget {
    fn new(sink: Box<dyn Fn(DropPayload)>) -> Self {
        Self {
            sink,
            accepted: Cell::new(false),
        }
    }
}

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(
        &self,
        pdataobj: Ref<IDataObject>,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> Result<()> {
        let accepted = pdataobj.ok().is_ok_and(accepts);
        self.accepted.set(accepted);
        // SAFETY: OLE passes a valid effect pointer.
        unsafe { *pdweffect = DROPEFFECT(choose_effect((*pdweffect).0, accepted)) };
        Ok(())
    }

    fn DragOver(
        &self,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> Result<()> {
        // SAFETY: OLE passes a valid effect pointer.
        unsafe { *pdweffect = DROPEFFECT(choose_effect((*pdweffect).0, self.accepted.get())) };
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        self.accepted.set(false);
        Ok(())
    }

    fn Drop(
        &self,
        pdataobj: Ref<IDataObject>,
        grfkeystate: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> Result<()> {
        self.accepted.set(false);
        // SAFETY: OLE passes a valid effect pointer.
        let allowed = unsafe { (*pdweffect).0 };
        unsafe { *pdweffect = DROPEFFECT(choose_effect(allowed, false)) };

        let Ok(data) = pdataobj.ok() else {
            return Ok(());
        };
        // Read now: the data object and any temp files behind it are only
        // promised to live until this call returns.
        let urls = extract_urls(data);
        if urls.is_empty() {
            return Ok(());
        }
        let (ctrl, shift) = modifiers(grfkeystate.0);
        (self.sink)(DropPayload { urls, ctrl, shift });
        // SAFETY: as above.
        unsafe { *pdweffect = DROPEFFECT(choose_effect(allowed, true)) };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, mem::ManuallyDrop, rc::Rc};
    use windows::{
        Win32::{
            Foundation::{DV_E_FORMATETC, E_NOTIMPL, S_OK},
            System::{
                Com::{IAdviseSink, IDataObject_Impl, IEnumFORMATETC, IEnumSTATDATA, STGMEDIUM_0},
                Memory::{GMEM_MOVEABLE, GlobalAlloc},
                Ole::{DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE, DROPEFFECT_NONE},
                SystemServices::{MK_CONTROL, MK_SHIFT},
            },
            UI::Shell::DROPFILES,
        },
        core::{BOOL, HRESULT},
    };

    /// A data object offering only the formats it was given.
    #[implement(IDataObject)]
    struct TestData {
        url: Option<String>,
        text: Option<String>,
        files: Vec<PathBuf>,
    }

    fn wide_bytes(text: &str) -> Vec<u8> {
        text.encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect()
    }

    fn hdrop_bytes(files: &[PathBuf]) -> Vec<u8> {
        let header = DROPFILES {
            pFiles: size_of::<DROPFILES>() as u32,
            fWide: BOOL(1),
            ..Default::default()
        };
        // SAFETY: DROPFILES is plain data.
        let mut bytes = unsafe {
            slice::from_raw_parts(&header as *const DROPFILES as *const u8, size_of::<DROPFILES>())
        }
        .to_vec();
        for file in files {
            let wide: Vec<u16> = std::os::windows::ffi::OsStrExt::encode_wide(file.as_os_str())
                .chain([0])
                .collect();
            bytes.extend(wide.into_iter().flat_map(u16::to_le_bytes));
        }
        bytes.extend([0, 0]);
        bytes
    }

    impl TestData {
        fn payload(&self, cf: u16) -> Option<Vec<u8>> {
            if cf == url_format() {
                self.url.as_deref().map(wide_bytes)
            } else if cf == CF_UNICODETEXT.0 {
                self.text.as_deref().map(wide_bytes)
            } else if cf == CF_HDROP.0 && !self.files.is_empty() {
                Some(hdrop_bytes(&self.files))
            } else {
                None
            }
        }
    }

    impl IDataObject_Impl for TestData_Impl {
        fn GetData(&self, pformatetcin: *const FORMATETC) -> Result<STGMEDIUM> {
            // SAFETY: the caller passes a valid FORMATETC.
            let cf = unsafe { (*pformatetcin).cfFormat };
            let bytes = self.payload(cf).ok_or(DV_E_FORMATETC)?;
            // SAFETY: the block is sized for the copy and handed to the caller,
            // which frees it through ReleaseStgMedium.
            unsafe {
                let handle = GlobalAlloc(GMEM_MOVEABLE, bytes.len())?;
                let ptr = GlobalLock(handle) as *mut u8;
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
                let _ = GlobalUnlock(handle);
                Ok(STGMEDIUM {
                    tymed: TYMED_HGLOBAL.0 as u32,
                    u: STGMEDIUM_0 { hGlobal: handle },
                    pUnkForRelease: ManuallyDrop::new(None),
                })
            }
        }

        fn GetDataHere(&self, _: *const FORMATETC, _: *mut STGMEDIUM) -> Result<()> {
            Err(E_NOTIMPL.into())
        }

        fn QueryGetData(&self, pformatetc: *const FORMATETC) -> HRESULT {
            // SAFETY: the caller passes a valid FORMATETC.
            let cf = unsafe { (*pformatetc).cfFormat };
            if self.payload(cf).is_some() { S_OK } else { DV_E_FORMATETC }
        }

        fn GetCanonicalFormatEtc(&self, _: *const FORMATETC, _: *mut FORMATETC) -> HRESULT {
            E_NOTIMPL
        }

        fn SetData(&self, _: *const FORMATETC, _: *const STGMEDIUM, _: BOOL) -> Result<()> {
            Err(E_NOTIMPL.into())
        }

        fn EnumFormatEtc(&self, _: u32) -> Result<IEnumFORMATETC> {
            Err(E_NOTIMPL.into())
        }

        fn DAdvise(&self, _: *const FORMATETC, _: u32, _: Ref<IAdviseSink>) -> Result<u32> {
            Err(E_NOTIMPL.into())
        }

        fn DUnadvise(&self, _: u32) -> Result<()> {
            Err(E_NOTIMPL.into())
        }

        fn EnumDAdvise(&self) -> Result<IEnumSTATDATA> {
            Err(E_NOTIMPL.into())
        }
    }

    fn data(url: Option<&str>, text: Option<&str>, files: &[PathBuf]) -> IDataObject {
        TestData {
            url: url.map(str::to_string),
            text: text.map(str::to_string),
            files: files.to_vec(),
        }
        .into()
    }

    /// A drop target whose payloads are collected in the returned cell.
    fn target() -> (IDropTarget, Rc<RefCell<Vec<DropPayload>>>) {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let sink = {
            let seen = seen.clone();
            move |payload| seen.borrow_mut().push(payload)
        };
        (DropTarget::new(Box::new(sink)).into(), seen)
    }

    fn all_effects() -> DROPEFFECT {
        DROPEFFECT(DROPEFFECT_COPY.0 | DROPEFFECT_MOVE.0 | DROPEFFECT_LINK.0)
    }

    /// Drag enters, then drops; returns (effect on enter, effect on drop).
    fn drag_and_drop(
        target: &IDropTarget,
        data: &IDataObject,
        keys: MODIFIERKEYS_FLAGS,
    ) -> (DROPEFFECT, DROPEFFECT) {
        let point = POINTL::default();
        let mut enter = all_effects();
        // SAFETY: direct calls on a target and data object of this test.
        unsafe { target.DragEnter(data, keys, point, &mut enter) }.unwrap();
        let mut over = all_effects();
        unsafe { target.DragOver(keys, point, &mut over) }.unwrap();
        assert_eq!(over, enter, "DragOver repeats the DragEnter verdict");
        let mut dropped = all_effects();
        unsafe { target.Drop(data, keys, point, &mut dropped) }.unwrap();
        (enter, dropped)
    }

    fn url_file(dir: &std::path::Path, name: &str, url: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, format!("[InternetShortcut]\r\nURL={url}\r\n")).unwrap();
        path
    }

    #[test]
    fn url_format_wins_over_text_and_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = url_file(dir.path(), "temp.url", "https://example.com/file");
        let data = data(
            Some("https://example.com/image.png"),
            Some("https://example.com/text"),
            &[file],
        );
        let (target, seen) = target();
        let (enter, dropped) = drag_and_drop(&target, &data, MODIFIERKEYS_FLAGS(0));
        assert_eq!((enter, dropped), (DROPEFFECT_COPY, DROPEFFECT_COPY));
        assert_eq!(
            *seen.borrow(),
            [DropPayload {
                urls: vec!["https://example.com/image.png".into()],
                ctrl: false,
                shift: false,
            }]
        );
    }

    #[test]
    fn text_with_two_urls_is_split() {
        let data = data(None, Some("see https://example.com/a\r\nand\r\nhttps://example.com/b"), &[]);
        let (target, seen) = target();
        drag_and_drop(&target, &data, MODIFIERKEYS_FLAGS(0));
        assert_eq!(
            seen.borrow()[0].urls,
            ["https://example.com/a", "https://example.com/b"]
        );
    }

    #[test]
    fn dropped_url_files_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let files = [
            url_file(dir.path(), "one.url", "https://example.com/1"),
            dir.path().join("note.txt"),
            url_file(dir.path(), "TWO.URL", "https://example.com/2"),
        ];
        fs::write(&files[1], "https://example.com/not-this").unwrap();
        let data = data(None, None, &files);
        let (target, seen) = target();
        let (enter, dropped) = drag_and_drop(&target, &data, MODIFIERKEYS_FLAGS(0));
        assert_eq!((enter, dropped), (DROPEFFECT_COPY, DROPEFFECT_COPY));
        assert_eq!(
            seen.borrow()[0].urls,
            ["https://example.com/1", "https://example.com/2"]
        );
    }

    #[test]
    fn huge_text_is_cut_but_keeps_leading_urls() {
        let text = format!("https://example.com/a
{}", "x".repeat(3 * MAX_TEXT_UNITS));
        let data = data(None, Some(&text), &[]);
        let read = read_wide_text(&data, CF_UNICODETEXT.0).unwrap();
        assert_eq!(read.encode_utf16().count(), MAX_TEXT_UNITS);
        assert!(read.starts_with("https://example.com/a
"));
    }

    #[test]
    fn text_file_alone_shows_no_drop() {
        let dir = tempfile::tempdir().unwrap();
        let txt = dir.path().join("note.txt");
        fs::write(&txt, "https://example.com/a").unwrap();
        let data = data(None, None, &[txt]);
        let (target, seen) = target();
        let (enter, dropped) = drag_and_drop(&target, &data, MODIFIERKEYS_FLAGS(0));
        assert_eq!((enter, dropped), (DROPEFFECT_NONE, DROPEFFECT_NONE));
        assert!(seen.borrow().is_empty());
    }

    #[test]
    fn text_without_a_url_is_accepted_but_adds_nothing() {
        let data = data(None, Some("just words"), &[]);
        let (target, seen) = target();
        let (enter, dropped) = drag_and_drop(&target, &data, MODIFIERKEYS_FLAGS(0));
        // The drag cannot tell before the drop whether text holds a URL.
        assert_eq!((enter, dropped), (DROPEFFECT_COPY, DROPEFFECT_NONE));
        assert!(seen.borrow().is_empty());
    }

    #[test]
    fn modifiers_reach_the_payload() {
        let data = data(Some("https://example.com/a"), None, &[]);
        let (target, seen) = target();
        drag_and_drop(&target, &data, MODIFIERKEYS_FLAGS(MK_CONTROL.0));
        drag_and_drop(&target, &data, MODIFIERKEYS_FLAGS(MK_CONTROL.0 | MK_SHIFT.0));
        let seen = seen.borrow();
        assert_eq!((seen[0].ctrl, seen[0].shift), (true, false));
        assert_eq!((seen[1].ctrl, seen[1].shift), (true, true));
    }

    #[test]
    fn move_only_source_is_not_answered_with_move() {
        let data = data(Some("https://example.com/a"), None, &[]);
        let (target, _) = target();
        let point = POINTL::default();
        let mut effect = DROPEFFECT_MOVE;
        // SAFETY: direct call on a target and data object of this test.
        unsafe { target.DragEnter(&data, MODIFIERKEYS_FLAGS(0), point, &mut effect) }.unwrap();
        assert_eq!(effect, DROPEFFECT_NONE);
    }
}
