use std::cell::RefCell;
use std::rc::Rc;

use crate::util::ResultExt;
use calloop::{EventLoop, LoopHandle};

use crate::platform::linux::LinuxClient;
use crate::platform::{LinuxCommon, PlatformWindow};
use crate::{CursorStyle, DisplayId, PlatformDisplay, WindowId, WindowParams};

pub struct HeadlessClientState {
    pub(crate) _loop_handle: LoopHandle<'static, HeadlessClient>,
    pub(crate) event_loop: Option<calloop::EventLoop<'static, HeadlessClient>>,
    pub(crate) common: LinuxCommon,
}

#[derive(Clone)]
pub(crate) struct HeadlessClient(Rc<RefCell<HeadlessClientState>>);

impl HeadlessClient {
    pub(crate) fn new() -> Self {
        let event_loop = EventLoop::try_new().unwrap();

        let (common, main_receiver) = LinuxCommon::new(event_loop.get_signal());

        let handle = event_loop.handle();

        handle
            .insert_source(main_receiver, |event, _, _: &mut HeadlessClient| {
                if let calloop::channel::Event::Msg(runnable) = event {
                    runnable.run();
                }
            })
            .ok();

        HeadlessClient(Rc::new(RefCell::new(HeadlessClientState { event_loop: Some(event_loop), _loop_handle: handle, common })))
    }
}

impl HeadlessClient {
    /// What other threads ask of the display thread is run as it comes
    /// (`crate::display`).
    pub(crate) fn take_commands<T: 'static>(&self, commands: calloop::channel::Channel<T>, mut run: impl FnMut(T) + 'static) {
        let handle = self.0.borrow()._loop_handle.clone();
        handle
            .insert_source(commands, move |event, _, _: &mut HeadlessClient| {
                if let calloop::channel::Event::Msg(command) = event {
                    run(command);
                }
            })
            .ok();
    }
}

impl LinuxClient for HeadlessClient {
    fn with_common<R>(&self, f: impl FnOnce(&mut LinuxCommon) -> R) -> R {
        f(&mut self.0.borrow_mut().common)
    }

    fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>> {
        vec![]
    }

    fn display(&self, _id: DisplayId) -> Option<Rc<dyn PlatformDisplay>> {
        None
    }

    #[cfg(feature = "screen-capture")]
    fn is_screen_capture_supported(&self) -> bool {
        false
    }

    #[cfg(feature = "screen-capture")]
    fn screen_capture_sources(&self) -> futures::channel::oneshot::Receiver<anyhow::Result<Vec<Rc<dyn crate::ScreenCaptureSource>>>> {
        let (mut tx, rx) = futures::channel::oneshot::channel();
        tx.send(Err(anyhow::anyhow!("Headless mode does not support screen capture."))).ok();
        rx
    }

    /// A window with no display (`HeadlessWindow`): laid out and drawn
    /// to nothing, for the layout lint's tests.
    fn open_window(&self, _handle: WindowId, params: WindowParams) -> anyhow::Result<Box<dyn PlatformWindow>> {
        Ok(Box::new(super::HeadlessWindow::new(params)))
    }

    fn set_cursor_style(&self, _style: CursorStyle) {}

    fn write_to_clipboard(&self, _item: crate::ClipboardItem) {}

    fn run(&self) {
        let mut event_loop = self.0.borrow_mut().event_loop.take().expect("App is already running");

        event_loop.run(None, &mut self.clone(), |_| {}).log_err();
    }
}
