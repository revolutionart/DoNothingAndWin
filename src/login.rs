//! A short-lived WebView2 process. Only Epic's final redirect response is read;
//! credentials and provider pages remain entirely inside the browser.
use crate::{
    epic::{Epic, SIGN_IN_URL, Session},
    storage,
};
use reqwest::Url;

pub fn is_code_page(address: &str) -> bool {
    Url::parse(address).is_ok_and(|u| {
        u.scheme() == "https"
            && u.host_str() == Some("www.epicgames.com")
            && u.port_or_known_default() == Some(443)
            && u.username().is_empty()
            && u.password().is_none()
            && u.path() == "/id/api/redirect"
    })
}

fn secure_site(address: &str) -> Option<String> {
    let u = Url::parse(address).ok()?;
    (u.scheme() == "https" && u.username().is_empty() && u.password().is_none())
        .then(|| u.host_str().map(str::to_owned))
        .flatten()
}

fn callback_code(json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    if !is_code_page(value.get("url")?.as_str()?) {
        return None;
    }
    let code = value.get("authorizationCode")?.as_str()?;
    (code.len() == 32 && code.bytes().all(|b| b.is_ascii_hexdigit())).then(|| code.to_owned())
}

/// The child writes its successful session through DPAPI. Neither the code nor
/// tokens are passed on the command line, stdout, or an unauthenticated socket.
pub fn connect() -> Result<Option<Session>, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let result = std::process::Command::new(exe)
        .arg("--epic-login")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("Could not open Epic sign-in: {e}"))?;
    match result.code() {
        Some(0) => storage::load_session().map(Some).ok_or_else(|| {
            "Epic sign-in finished but the encrypted session could not be loaded.".into()
        }),
        Some(2) => Ok(None),
        _ => Err("Epic sign-in did not complete. Please try Connect account again.".into()),
    }
}

enum LoginEvent {
    Navigate(String),
    Loading(String),
    Loaded(String),
    Code(Option<String>),
}

pub fn run() -> Result<bool, String> {
    use tao::{
        dpi::LogicalSize,
        event::{Event, WindowEvent},
        event_loop::{ControlFlow, EventLoopBuilder},
        platform::run_return::EventLoopExtRunReturn,
        window::{Theme, WindowBuilder},
    };
    use wry::{
        NewWindowResponse, PageLoadEvent, Rect, WebContext, WebViewBuilder,
        dpi::{LogicalPosition, LogicalSize as WebSize},
    };
    let mut events = EventLoopBuilder::<LoginEvent>::with_user_event().build();
    let window = WindowBuilder::new()
        .with_title("Sign in to Epic Games — DoNothing&Win Launcher")
        .with_theme(Some(Theme::Dark))
        .with_inner_size(LogicalSize::new(650., 960.))
        .with_min_inner_size(LogicalSize::new(460., 600.))
        .build(&events)
        .map_err(|e| e.to_string())?;
    let bounds = |width: f64, height: f64, top: f64| Rect {
        position: LogicalPosition::new(0., top).into(),
        size: WebSize::new(width, (height - top).max(1.)).into(),
    };
    // Private browsing avoids retaining another user's Epic/provider cookies.
    // This profile is separate from every installed browser and Epic Launcher.
    let mut context = WebContext::new(Some(storage::data_dir().join("SignInBrowser")));
    let bar = WebViewBuilder::new_with_web_context(&mut context).with_incognito(true)
        .with_devtools(false).with_bounds(bounds(650., 40., 0.))
        .with_html(r#"<!doctype html><meta charset="utf-8"><style>html{color-scheme:dark}body{margin:0;background:#18191b;color:#eee;font:16px 'Segoe UI',sans-serif;padding:8px 14px;white-space:nowrap;overflow:hidden}span{color:#8c949e}</style><span>HTTPS · </span><b id="site">www.epicgames.com</b>"#)
        .with_navigation_handler(|url| url == "about:blank").build_as_child(&window).map_err(|_| "Microsoft Edge WebView2 Runtime is required for Epic sign-in. Install the Evergreen Runtime from Microsoft, then try again.".to_owned())?;
    let nav_proxy = events.create_proxy();
    let page_proxy = events.create_proxy();
    let popup_proxy = events.create_proxy();
    let browser = WebViewBuilder::new_with_web_context(&mut context)
        .with_incognito(true)
        .with_devtools(false)
        .with_background_color((15, 16, 19, 255))
        .with_bounds(bounds(650., 960., 40.))
        .with_url(SIGN_IN_URL)
        .with_navigation_handler(move |url| {
            if secure_site(&url).is_none() {
                return false;
            }
            let _ = nav_proxy.send_event(LoginEvent::Loading(url));
            true
        })
        .with_new_window_req_handler(move |url, _| {
            if secure_site(&url).is_some() {
                let _ = popup_proxy.send_event(LoginEvent::Navigate(url));
            }
            NewWindowResponse::Deny
        })
        .with_download_started_handler(|_, _| false)
        .with_on_page_load_handler(move |event, url| {
            if matches!(event, PageLoadEvent::Finished) {
                let _ = page_proxy.send_event(LoginEvent::Loaded(url));
            }
        })
        .build_as_child(&window)
        .map_err(|e| format!("Could not create the Epic sign-in browser: {e}"))?;
    let mut code = None;
    let mut callback_error = false;
    let mut reading = false;
    let callback_proxy = events.create_proxy();
    events.run_return(|event,_,flow| {
        *flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => *flow = ControlFlow::Exit,
            Event::WindowEvent { event: WindowEvent::Resized(_), .. }
            | Event::WindowEvent { event: WindowEvent::ScaleFactorChanged { .. }, .. } => {
                let size = window.inner_size().to_logical::<f64>(window.scale_factor());
                let _ = bar.set_bounds(bounds(size.width,40.,0.));
                let _ = browser.set_bounds(bounds(size.width,size.height,40.));
            }
            Event::UserEvent(LoginEvent::Navigate(url)) => { let _ = browser.load_url(&url); }
            Event::UserEvent(LoginEvent::Loading(url)) => {
                if let Some(site) = secure_site(&url) {
                    let label = serde_json::to_string(&site).unwrap_or_default();
                    let _ = bar.evaluate_script(&format!("document.getElementById('site').textContent={label}"));
                    window.set_title(&format!("{site} — Sign in to Epic Games"));
                }
                if is_code_page(&url) {
                    let _ = browser.set_visible(false);
                    window.set_title("Connecting your Epic account…");
                }
            }
            Event::UserEvent(LoginEvent::Loaded(url)) if is_code_page(&url) && !reading => {
                reading = true;
                let proxy = callback_proxy.clone();
                // Re-check location inside the script, avoiding a navigation race.
                // No script is injected into the sign-in or provider forms.
                let script = r#"(()=>{if(location.origin!=='https://www.epicgames.com'||location.pathname!=='/id/api/redirect')return null;try{const data=JSON.parse(document.body.innerText);return {url:location.href,authorizationCode:data.authorizationCode}}catch{return null}})()"#;
                if browser.evaluate_script_with_callback(script,move |value| {
                    let _ = proxy.send_event(LoginEvent::Code(callback_code(&value)));
                }).is_err() { callback_error = true; *flow = ControlFlow::Exit; }
            }
            Event::UserEvent(LoginEvent::Code(result)) => { callback_error = result.is_none(); code=result; *flow=ControlFlow::Exit; }
            _ => {}
        }
    });
    drop(browser);
    drop(bar);
    drop(window);
    if callback_error {
        return Err(
            "Epic did not return a valid sign-in response. Please try signing in again.".into(),
        );
    }
    let Some(code) = code else {
        return Ok(false);
    };
    let _lock = crate::epic::session_lock()?;
    let session = Epic::new()?.sign_in(&code)?;
    storage::save_session(&session)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn callback_requires_exact_https_origin_and_path() {
        assert!(is_code_page(
            "https://www.epicgames.com/id/api/redirect?clientId=abc"
        ));
        for url in [
            "http://www.epicgames.com/id/api/redirect",
            "https://www.epicgames.com.evil.test/id/api/redirect",
            "https://www.epicgames.com/id/api/redirect/evil",
            "https://www.epicgames.com:444/id/api/redirect",
            "https://evil@www.epicgames.com/id/api/redirect",
        ] {
            assert!(!is_code_page(url), "{url}");
        }
    }
    #[test]
    fn callback_rejects_malformed_codes_and_other_pages() {
        let good = serde_json::json!({"url":"https://www.epicgames.com/id/api/redirect", "authorizationCode":"0123456789abcdef0123456789abcdef"});
        assert!(callback_code(&good.to_string()).is_some());
        let mut bad = good.clone();
        bad["url"] = "https://www.epicgames.com/id/login".into();
        assert!(callback_code(&bad.to_string()).is_none());
        let mut bad = good;
        bad["authorizationCode"] = "<script>".into();
        assert!(callback_code(&bad.to_string()).is_none());
        assert!(callback_code("null").is_none());
        assert!(secure_site("javascript:alert(1)").is_none());
        assert_eq!(
            secure_site("https://accounts.google.com/login").as_deref(),
            Some("accounts.google.com")
        );
    }
}
