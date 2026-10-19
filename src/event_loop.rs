use winit::{
    application::ApplicationHandler,
    event::{DeviceEvent, DeviceId, StartCause, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::WindowId,
};

pub fn run(
    name: &str,
    options: eframe::NativeOptions,
    creator: eframe::AppCreator<'static>,
) -> eframe::Result {
    let events = EventLoop::<eframe::UserEvent>::with_user_event().build()?;
    let mut app = BackgroundAwareApp {
        inner: eframe::create_native(name, options, creator, &events),
        window: None,
    };
    events.run_app(&mut app)?;
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    use std::time::{Duration, Instant};
    use winit::platform::windows::EventLoopBuilderExtWindows;

    // Run explicitly on a Windows desktop; uses an isolated blank app with no
    // account, downloads, settings or project paths.
    #[test]
    #[ignore = "Requires a Windows desktop and creates a temporary native window"]
    fn background_window_processes_workers_and_exits_cleanly() {
        struct Probe {
            completed: Arc<AtomicBool>,
            hidden_frames: Arc<AtomicUsize>,
            observed: Arc<AtomicBool>,
            exited: Arc<AtomicBool>,
            hwnd: isize,
        }
        impl eframe::App for Probe {
            fn on_exit(&mut self) {
                self.exited.store(true, Ordering::Release);
            }
            fn update(&mut self, ctx: &eframe::egui::Context, _: &mut eframe::Frame) {
                if crate::window_frame::is_backgrounded() {
                    self.hidden_frames.fetch_add(1, Ordering::Relaxed);
                } else {
                    if crate::window_frame::tray_enabled() {
                        panic!("tray startup unexpectedly showed the window");
                    } else {
                        assert!(!crate::window_frame::hide_to_tray());
                        ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Minimized(true));
                    }
                }
                if self.completed.load(Ordering::Acquire) {
                    self.observed.store(true, Ordering::Release);
                    if crate::window_frame::tray_enabled() {
                        // Exercise the actual tray click handler before hiding and exiting.
                        unsafe {
                            use windows_sys::Win32::UI::WindowsAndMessaging::*;
                            SendMessageW(self.hwnd as _, WM_APP + 77, 0, WM_LBUTTONUP as isize);
                        }
                        assert!(!crate::window_frame::is_backgrounded());
                        assert!(!crate::window_frame::tray_present());
                        assert!(crate::window_frame::hide_to_tray());
                        crate::window_frame::request_exit();
                    } else {
                        crate::window_frame::completion_notice("Portable completion test");
                        assert!(!crate::window_frame::tray_present());
                        assert!(crate::window_frame::close_button());
                    }
                }
                // After periodic hidden frames succeed, only the worker should
                // wake us before the watchdog's three-second deadline.
                ctx.request_repaint_after(if self.hidden_frames.load(Ordering::Relaxed) < 3 {
                    Duration::from_millis(100)
                } else {
                    Duration::from_secs(10)
                });
            }
        }
        struct Harness<'a> {
            app: BackgroundAwareApp<'a>,
            cycles: usize,
            hidden_cycles: usize,
            hidden_polls: usize,
            start: Instant,
        }
        impl ApplicationHandler<eframe::UserEvent> for Harness<'_> {
            fn resumed(&mut self, e: &ActiveEventLoop) {
                self.app.resumed(e);
            }
            fn window_event(&mut self, e: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
                self.app.window_event(e, id, event);
            }
            fn user_event(&mut self, e: &ActiveEventLoop, event: eframe::UserEvent) {
                self.app.user_event(e, event);
            }
            fn new_events(&mut self, e: &ActiveEventLoop, cause: StartCause) {
                self.cycles += 1;
                if crate::window_frame::is_backgrounded() {
                    self.hidden_cycles += 1;
                    if e.control_flow() == ControlFlow::Poll {
                        self.hidden_polls += 1;
                    }
                }
                if self.start.elapsed() > Duration::from_secs(3) {
                    e.exit();
                }
                self.app.new_events(e, cause);
            }
            fn about_to_wait(&mut self, e: &ActiveEventLoop) {
                self.app.about_to_wait(e);
            }
            fn exiting(&mut self, e: &ActiveEventLoop) {
                self.app.exiting(e);
            }
        }
        let events = EventLoop::<eframe::UserEvent>::with_user_event()
            .with_any_thread(true)
            .build()
            .unwrap();
        let completed = Arc::new(AtomicBool::new(false));
        let observed = Arc::new(AtomicBool::new(false));
        let hidden_frames = Arc::new(AtomicUsize::new(0));
        let exited = Arc::new(AtomicBool::new(false));
        let mut probe = Probe {
            completed: completed.clone(),
            observed: observed.clone(),
            hidden_frames: hidden_frames.clone(),
            exited: exited.clone(),
            hwnd: 0,
        };
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let inner = eframe::create_native(
            "Tray idle regression probe",
            eframe::NativeOptions::default(),
            Box::new(move |cc| {
                crate::window_frame::install(cc);
                use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                if let RawWindowHandle::Win32(window) = cc.window_handle().unwrap().as_raw() {
                    probe.hwnd = window.hwnd.get();
                }
                assert_eq!(
                    crate::window_frame::start_in_tray(),
                    crate::window_frame::tray_enabled()
                );
                let ctx = cc.egui_ctx.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(800));
                    completed.store(true, Ordering::Release);
                    ctx.request_repaint();
                    if done_rx.recv_timeout(Duration::from_secs(3)).is_err() {
                        ctx.request_repaint();
                    }
                });
                Ok(Box::new(probe))
            }),
            &events,
        );
        let mut harness = Harness {
            app: BackgroundAwareApp {
                inner,
                window: None,
            },
            cycles: 0,
            hidden_cycles: 0,
            hidden_polls: 0,
            start: Instant::now(),
        };
        events.run_app(&mut harness).unwrap();
        let _ = done_tx.send(());
        assert!(
            exited.load(Ordering::Acquire),
            "normal on_exit/save hook was skipped"
        );
        assert!(
            !crate::window_frame::tray_present(),
            "tray icon survived shutdown"
        );
        assert!(
            observed.load(Ordering::Acquire),
            "worker completion was not processed while hidden"
        );
        assert!(
            hidden_frames.load(Ordering::Relaxed) >= 3,
            "scheduled hidden frames stalled"
        );
        eprintln!(
            "cycles {}, hidden {}, hidden polls {}, frames {}, elapsed {:?}",
            harness.cycles,
            harness.hidden_cycles,
            harness.hidden_polls,
            hidden_frames.load(Ordering::Relaxed),
            harness.start.elapsed()
        );
        assert!(
            harness.hidden_cycles < 500,
            "hidden event loop did not sleep"
        );
    }
}

struct BackgroundAwareApp<'a> {
    inner: eframe::EframeWinitApplication<'a>,
    window: Option<WindowId>,
}

impl ApplicationHandler<eframe::UserEvent> for BackgroundAwareApp<'_> {
    fn resumed(&mut self, events: &ActiveEventLoop) {
        self.inner.resumed(events);
    }

    fn window_event(&mut self, events: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        self.window.get_or_insert(id);
        self.inner.window_event(events, id, event);
    }

    fn new_events(&mut self, events: &ActiveEventLoop, cause: StartCause) {
        self.inner.new_events(events, cause);
    }

    fn user_event(&mut self, events: &ActiveEventLoop, event: eframe::UserEvent) {
        self.inner.user_event(events, event);
    }

    fn device_event(&mut self, events: &ActiveEventLoop, id: DeviceId, event: DeviceEvent) {
        self.inner.device_event(events, id, event);
    }

    fn about_to_wait(&mut self, events: &ActiveEventLoop) {
        self.inner.about_to_wait(events);
        if crate::window_frame::take_exit_request() && !events.exiting() {
            if let Some(id) = self.window {
                // Do not wait for Windows to paint a hidden/minimized window.
                // Route through eframe so it saves and destroys the app normally.
                self.inner
                    .window_event(events, id, WindowEvent::CloseRequested);
                if !events.exiting() {
                    self.inner
                        .window_event(events, id, WindowEvent::RedrawRequested);
                }
            }
            return;
        }
        // eframe 0.33 schedules a redraw by switching to Poll. Windows does not
        // dispatch WM_PAINT for SW_HIDE windows, so it otherwise spins forever.
        // Service the due frame explicitly (Workspace skips invisible UI) and
        // return to event-driven waiting. Worker messages and repaint deadlines
        // still wake the app to process transfers, notifications and tray actions.
        if crate::window_frame::is_backgrounded()
            && events.control_flow() == ControlFlow::Poll
            && !events.exiting()
        {
            events.set_control_flow(ControlFlow::Wait);
            if let Some(id) = self.window {
                self.inner
                    .window_event(events, id, WindowEvent::RedrawRequested);
            }
        }
    }

    fn suspended(&mut self, events: &ActiveEventLoop) {
        self.inner.suspended(events);
    }

    fn exiting(&mut self, events: &ActiveEventLoop) {
        crate::window_frame::remove_tray_icon();
        self.inner.exiting(events);
    }

    fn memory_warning(&mut self, events: &ActiveEventLoop) {
        self.inner.memory_warning(events);
    }
}
