//! The one-shot listener the browser returns to after sign-in (RFC 8252 §7.3):
//! 127.0.0.1 on a port the OS picks, one request, then closed.

use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub const TIMED_OUT: &str = "timed_out";

/// How long the person has to finish signing in in the browser.
pub const SIGN_IN_WINDOW: Duration = Duration::from_secs(300);

pub struct Loopback {
    listener: TcpListener,
    pub redirect_uri: String,
}

impl Loopback {
    pub async fn bind() -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        Ok(Self {
            listener,
            redirect_uri: format!("http://127.0.0.1:{port}/callback"),
        })
    }

    /// Wait for the browser's request to `/callback`, answer it with a short
    /// bilingual page, and hand back its target (path and query). Requests for
    /// anything else (a favicon) are answered 404 and the wait goes on.
    pub async fn receive(self, window: Duration) -> Result<String, String> {
        tokio::time::timeout(window, async move {
            loop {
                let (mut stream, _) = self.listener.accept().await.map_err(|e| e.to_string())?;
                let mut buffer = vec![0u8; 8192];
                let read = stream.read(&mut buffer).await.map_err(|e| e.to_string())?;
                let request = String::from_utf8_lossy(&buffer[..read]);
                let target = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("")
                    .to_string();
                if !target.starts_with("/callback") {
                    let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                    continue;
                }
                let ok = target.contains("code=");
                let body = page(ok);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
                return Ok(target);
            }
        })
        .await
        .map_err(|_| TIMED_OUT.to_string())?
    }
}

/// The page the browser shows after the redirect, in English and Arabic,
/// since the app's language isn't known to the browser.
fn page(ok: bool) -> String {
    let (en, ar) = if ok {
        (
            "You're signed in. You can close this tab and go back to Whity Agent.",
            "تم تسجيل الدخول. يمكنك إغلاق هذه الصفحة والعودة إلى وكيل Whity.",
        )
    } else {
        (
            "Sign-in didn't finish. Go back to Whity Agent and try again.",
            "لم يكتمل تسجيل الدخول. عُد إلى وكيل Whity وحاول مرة أخرى.",
        )
    };
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Whity Agent</title>\
<style>body{{font-family:'Noto Sans','Noto Sans Arabic',system-ui,sans-serif;font-size:16px;margin:48px auto;max-width:520px;padding:0 16px}}p{{margin:0 0 16px}}</style>\
</head><body><p lang=\"en\" dir=\"ltr\">{en}</p><p lang=\"ar\" dir=\"rtl\">{ar}</p></body></html>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn it_returns_the_callback_target_and_skips_other_requests() {
        let loopback = Loopback::bind().await.unwrap();
        let port = loopback
            .redirect_uri
            .split(':')
            .nth(2)
            .unwrap()
            .split('/')
            .next()
            .unwrap()
            .to_string();
        let waiter = tokio::spawn(loopback.receive(Duration::from_secs(5)));

        let client = reqwest::Client::new();
        let favicon = client
            .get(format!("http://127.0.0.1:{port}/favicon.ico"))
            .send()
            .await
            .unwrap();
        assert_eq!(favicon.status(), 404);
        let page = client
            .get(format!("http://127.0.0.1:{port}/callback?code=abc&state=s"))
            .send()
            .await
            .unwrap();
        assert!(page.text().await.unwrap().contains("You're signed in"));

        assert_eq!(waiter.await.unwrap().unwrap(), "/callback?code=abc&state=s");
    }

    #[tokio::test]
    async fn it_gives_up_after_the_window() {
        let loopback = Loopback::bind().await.unwrap();
        assert_eq!(
            loopback
                .receive(Duration::from_millis(50))
                .await
                .unwrap_err(),
            TIMED_OUT
        );
    }
}
