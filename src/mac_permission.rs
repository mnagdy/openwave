//! Request microphone authorization before constructing a CoreAudio input unit.
use anyhow::{bail, Context, Result};
use block2::RcBlock;
use objc2::{
    class, msg_send,
    runtime::{AnyClass, AnyObject, Bool},
};
use std::{sync::mpsc, time::Duration};

#[link(name = "AVFoundation", kind = "framework")]
extern "C" {
    static AVMediaTypeAudio: *mut AnyObject;
}

pub fn microphone() -> Result<()> {
    // The framework owns AVMediaTypeAudio; Objective-C calls borrow it. The
    // request API copies the completion block, so a delayed reply remains valid.
    let pending = objc2::rc::autoreleasepool(|_| unsafe {
        let media = AVMediaTypeAudio;
        let capture =
            AnyClass::get(c"AVCaptureDevice").context("Loading macOS microphone authorization")?;
        let status: isize = msg_send![capture, authorizationStatusForMediaType: media];
        match status {
            3 => Ok(None),
            1 | 2 => {
                bail!("Microphone access is disabled. Allow OpenRoom in System Settings → Privacy & Security → Microphone.");
            }
            0 => {
                let bundle: *mut AnyObject = msg_send![class!(NSBundle), mainBundle];
                let key: *mut AnyObject = msg_send![class!(NSString), stringWithUTF8String: c"NSMicrophoneUsageDescription".as_ptr()];
                let description: *mut AnyObject =
                    msg_send![bundle, objectForInfoDictionaryKey: key];
                if description.is_null() {
                    bail!("Microphone permission has not been granted. Use the packaged OpenRoom.app to request access.");
                }
                let (tx, rx) = mpsc::channel();
                let callback = RcBlock::new(move |granted: Bool| {
                    let _ = tx.send(granted.as_bool());
                });
                let _: () = msg_send![capture, requestAccessForMediaType: media, completionHandler: &*callback];
                Ok(Some(rx))
            }
            _ => bail!("macOS returned an unknown microphone authorization status: {status}"),
        }
    })?;
    if let Some(rx) = pending {
        let granted = rx.recv_timeout(Duration::from_secs(60)).context(
            "Waiting for microphone permission; respond to the macOS permission prompt and retry",
        )?;
        if !granted {
            bail!("Microphone access was declined. Allow OpenRoom in System Settings → Privacy & Security → Microphone.");
        }
    }
    Ok(())
}
