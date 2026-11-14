//! Android entry point and platform services. Android owns installation and updates.
use android_activity::AndroidApp;
use anyhow::{Context, Result};
use jni::{
    JavaVM,
    objects::{JObject, JValue},
};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

static APP: Mutex<Option<AndroidApp>> = Mutex::new(None);
static RESUMED: AtomicBool = AtomicBool::new(false);
static SCAN_CANCEL: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);

#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info),
    );
    *APP.lock().expect("Android context lock") = Some(app.clone());
    let options = eframe::NativeOptions {
        android_app: Some(app),
        ..Default::default()
    };
    if let Err(error) = eframe::run_native(
        "Garage Distiller",
        options,
        Box::new(|cc| Ok(Box::new(crate::ui::GarageApp::new(cc)))),
    ) {
        log::error!("Unable to start Garage Distiller: {error}");
    }
    *APP.lock().expect("Android context lock") = None;
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_francoispeyret_garagedistiller_MainActivity_setResumed(
    _env: jni::JNIEnv<'_>,
    _class: jni::objects::JClass<'_>,
    resumed: jni::sys::jboolean,
) {
    RESUMED.store(resumed != 0, Ordering::Relaxed);
    if resumed == 0
        && let Ok(cancel) = SCAN_CANCEL.lock()
        && let Some(cancel) = cancel.as_ref()
    {
        cancel.store(true, Ordering::Relaxed);
    }
}

pub fn register_scan(cancel: Arc<AtomicBool>) {
    *SCAN_CANCEL.lock().expect("Android scan lock") = Some(cancel);
}

pub fn is_resumed() -> bool {
    RESUMED.load(Ordering::Relaxed)
}

pub fn data_path() -> Result<PathBuf> {
    APP.lock()
        .expect("Android context lock")
        .as_ref()
        .and_then(AndroidApp::internal_data_path)
        .map(|path| path.join("state.json"))
        .context("Le stockage privé Android est indisponible")
}

pub fn notify(body: &str) -> Result<()> {
    let app = APP
        .lock()
        .expect("Android context lock")
        .clone()
        .context("L’activité Android est indisponible")?;
    // AndroidApp keeps these VM/global activity references alive for this call.
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) }?;
    let mut env = vm.attach_current_thread()?;
    env.with_local_frame(8, |env| -> jni::errors::Result<()> {
        let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
        let message = env.new_string(body)?;
        let result = env.call_method(
            &activity,
            "notifyOffer",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&message)],
        );
        if env.exception_check()? {
            env.exception_describe()?;
            env.exception_clear()?;
        }
        result.map(|_| ())
    })
    .context("Notification Android impossible")
}

#[derive(Clone)]
struct InputBridge {
    id: eframe::egui::Id,
    text: String,
    start: usize,
    end: usize,
}

/// winit 0.30 consumes GameActivity TextEvent without forwarding it to egui.
/// Synchronize full text and selections, including IME composition, for the focused field.
pub fn sync_text(
    ui: &eframe::egui::Ui,
    text: &mut String,
    output: &mut eframe::egui::text_edit::TextEditOutput,
    password: bool,
    numeric: bool,
) {
    use android_activity::input::{TextInputState, TextSpan};
    use eframe::egui::{
        Id,
        text::{CCursor, CCursorRange},
    };
    if !output.response.has_focus() {
        return;
    }
    let Some(app) = APP.lock().expect("Android context lock").clone() else {
        return;
    };
    let key = Id::new("android_text_bridge");
    let previous = ui.ctx().data(|data| data.get_temp::<InputBridge>(key));
    let native = app.text_input_state();
    let new_focus = output.response.gained_focus()
        || previous
            .as_ref()
            .is_none_or(|state| state.id != output.response.id);
    let native_changed = previous.as_ref().is_some_and(|last| {
        last.text != native.text
            || last.start != native.selection.start
            || last.end != native.selection.end
    });
    if !new_focus && native_changed && !output.response.changed() {
        if *text != native.text {
            *text = native.text.clone();
            output.response.mark_changed();
        }
        output.state.cursor.set_char_range(Some(CCursorRange::two(
            CCursor::new(crate::text_input::utf16_to_chars(
                text,
                native.selection.start,
            )),
            CCursor::new(crate::text_input::utf16_to_chars(
                text,
                native.selection.end,
            )),
        )));
        output.state.clone().store(ui.ctx(), output.response.id);
    } else if new_focus || output.response.changed() || output.response.clicked() {
        let range = output
            .state
            .cursor
            .char_range()
            .unwrap_or(CCursorRange::one(CCursor::new(text.chars().count())));
        app.set_text_input_state(TextInputState {
            text: text.clone(),
            selection: TextSpan {
                start: crate::text_input::chars_to_utf16(text, range.secondary.index),
                end: crate::text_input::chars_to_utf16(text, range.primary.index),
            },
            compose_region: None,
        });
        // Android input type: password, number, or plain text.
        app.set_ime_editor_info(
            android_activity::input::InputType::from_bits_retain(if password {
                0x81
            } else if numeric {
                2
            } else {
                1
            }),
            android_activity::input::TextInputAction::Done,
            android_activity::input::ImeOptions::IMG_FLAG_NO_EXTRACT_UI,
        );
    }
    ui.ctx().data_mut(|data| {
        data.insert_temp(
            key,
            InputBridge {
                id: output.response.id,
                text: native.text,
                start: native.selection.start,
                end: native.selection.end,
            },
        )
    });
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(50));
}

static KEYBOARD_INSET: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
#[unsafe(no_mangle)]
pub extern "system" fn Java_fr_francoispeyret_garagedistiller_MainActivity_setKeyboardInset(
    _env: jni::JNIEnv<'_>,
    _class: jni::objects::JClass<'_>,
    pixels: jni::sys::jint,
) {
    KEYBOARD_INSET.store(pixels.max(0), Ordering::Relaxed);
}

pub fn resize_for_keyboard(ctx: &eframe::egui::Context, input: &mut eframe::egui::RawInput) {
    if let Some(rect) = input.screen_rect.as_mut() {
        rect.max.y = (rect.max.y
            - KEYBOARD_INSET.load(Ordering::Relaxed) as f32 / ctx.pixels_per_point())
        .max(rect.min.y + 100.0);
    }
}
