use std::time::Duration;

use reqwest::{redirect::Policy, Client};

const MANTICORE_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) fn build_manticore_http_client() -> reqwest::Result<Client> {
    build_manticore_http_client_with_timeout(MANTICORE_REQUEST_TIMEOUT)
}

fn build_manticore_http_client_with_timeout(timeout: Duration) -> reqwest::Result<Client> {
    Client::builder()
        .redirect(Policy::none())
        .timeout(timeout)
        .build()
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        time::Duration,
    };

    use reqwest::StatusCode;
    use tokio::{io::AsyncWriteExt, net::TcpListener, time::sleep};

    use super::*;

    #[tokio::test]
    async fn client_does_not_follow_redirects() {
        let target_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_addr = target_listener.local_addr().unwrap();
        let target_hit = Arc::new(AtomicBool::new(false));
        let target_hit_for_task = target_hit.clone();
        let target_task = tokio::spawn(async move {
            if let Ok(Ok((mut socket, _))) =
                tokio::time::timeout(Duration::from_millis(250), target_listener.accept()).await
            {
                target_hit_for_task.store(true, Ordering::SeqCst);
                let _ = socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                    .await;
            }
        });

        let redirect_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let redirect_addr = redirect_listener.local_addr().unwrap();
        let redirect_task = tokio::spawn(async move {
            let (mut socket, _) = redirect_listener.accept().await.unwrap();
            let response = format!(
                "HTTP/1.1 302 Found\r\nLocation: http://{target_addr}/redirected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });

        let client = build_manticore_http_client_with_timeout(Duration::from_secs(1)).unwrap();
        let response = client
            .get(format!("http://{redirect_addr}/start"))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FOUND);

        redirect_task.await.unwrap();
        target_task.await.unwrap();
        assert!(!target_hit.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn client_times_out_stalled_responses() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_task = tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.unwrap();
            sleep(Duration::from_secs(1)).await;
        });

        let client = build_manticore_http_client_with_timeout(Duration::from_millis(50)).unwrap();
        let error = client
            .get(format!("http://{addr}/stalled"))
            .send()
            .await
            .expect_err("stalled Manticore response must time out");

        assert!(error.is_timeout(), "expected timeout error, got {error}");
        server_task.abort();
    }
}
