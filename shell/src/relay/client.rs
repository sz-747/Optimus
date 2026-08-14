use std::time::Duration;

use reqwest::blocking::{Client, Response};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::Deserialize;

use super::model::{RelayAck, RelayCommand, RelayConfig, SnapshotEnvelope};
use super::RelayError;

pub trait RelayTransport: Send + Sync {
    fn publish(&self, snapshot: &SnapshotEnvelope) -> Result<(), RelayError>;
    fn poll(&self) -> Result<Vec<RelayCommand>, RelayError>;
    fn acknowledge(
        &self,
        command: &RelayCommand,
        status: &str,
        error_code: Option<&str>,
    ) -> Result<(), RelayError>;
}

pub struct HttpRelayClient {
    client: Client,
    base_url: String,
    consumer_id: String,
}

impl HttpRelayClient {
    pub fn new(config: &RelayConfig) -> Result<Self, RelayError> {
        let mut headers = HeaderMap::new();
        let mut authorization = HeaderValue::from_str(&format!("Bearer {}", config.desktop_token))
            .map_err(|_| RelayError::Config("desktop token is not a valid header".to_string()))?;
        authorization.set_sensitive(true);
        headers.insert(AUTHORIZATION, authorization);
        headers.insert(
            "x-optimus-device",
            HeaderValue::from_str(&config.device_id)
                .map_err(|_| RelayError::Config("device id is not a valid header".to_string()))?,
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let client = Client::builder()
            .default_headers(headers)
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(10))
            .user_agent("optimus-desktop-relay/1")
            .build()
            .map_err(|error| RelayError::Http(error.to_string()))?;
        Ok(Self {
            client,
            base_url: config.base_url.clone(),
            consumer_id: config.consumer_id.clone(),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }
}

impl RelayTransport for HttpRelayClient {
    fn publish(&self, snapshot: &SnapshotEnvelope) -> Result<(), RelayError> {
        checked(
            self.client
                .put(self.url("/api/control-plane/desktop/snapshot"))
                .json(snapshot)
                .send(),
        )?;
        Ok(())
    }

    fn poll(&self) -> Result<Vec<RelayCommand>, RelayError> {
        #[derive(Deserialize)]
        struct PollResponse {
            commands: Vec<RelayCommand>,
        }
        let response = checked(
            self.client
                .get(self.url("/api/control-plane/desktop/commands"))
                .query(&[("consumer", &self.consumer_id)])
                .send(),
        )?;
        response
            .json::<PollResponse>()
            .map(|value| value.commands)
            .map_err(|error| RelayError::Http(format!("invalid command response: {error}")))
    }

    fn acknowledge(
        &self,
        command: &RelayCommand,
        status: &str,
        error_code: Option<&str>,
    ) -> Result<(), RelayError> {
        let acknowledgement = RelayAck {
            command_id: &command.command_id,
            lease_id: &command.lease_id,
            status,
            error_code,
        };
        checked(
            self.client
                .patch(self.url("/api/control-plane/desktop/commands"))
                .query(&[("consumer", &self.consumer_id)])
                .json(&acknowledgement)
                .send(),
        )?;
        Ok(())
    }
}

fn checked(result: Result<Response, reqwest::Error>) -> Result<Response, RelayError> {
    let response = result.map_err(|error| RelayError::Http(error.to_string()))?;
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    Err(RelayError::Http(format!(
        "server returned {}",
        status.as_u16()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay::model::{
        RelayCommandKind, RelayProviderConfig, RelayWorkspaceConfig, RemoteCapacity, RemoteCatalog,
        RemoteSnapshot, RemoteTotals,
    };
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    #[test]
    fn real_http_client_publishes_polls_and_acknowledges_with_desktop_identity() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);
        let server = std::thread::spawn(move || {
            for index in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_request(&mut stream);
                captured.lock().unwrap().push(request);
                let body = if index == 1 {
                    r#"{"commands":[{"commandId":"12345678-90ab-4000-8000-000000000000","deviceId":"primary","type":"start_run","payload":{"workspaceId":"workspace-primary","profileId":"codex-subscription","sessionName":"Run","agents":[{"agentName":"Agent","task":"Build","kind":"writer","fileScope":["ui"]}]},"status":"leased","issuedAt":1,"expiresAt":9999999999999,"leaseId":"11111111-1111-4111-8111-111111111111","leaseOwner":"22222222-2222-4222-8222-222222222222","leaseUntil":9999999999999,"errorCode":null,"updatedAt":1}]}"#
                } else {
                    "{}"
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            }
        });
        let config = RelayConfig {
            base_url: format!("http://{address}"),
            desktop_token: "desktop-token-at-least-24-characters".to_string(),
            device_id: "primary".to_string(),
            instance_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_string(),
            consumer_id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb".to_string(),
            workspaces: vec![RelayWorkspaceConfig {
                id: "workspace-primary".to_string(),
                label: "Primary".to_string(),
                repo_root: "C:/repo".to_string(),
            }],
            providers: vec![RelayProviderConfig {
                id: "codex-subscription".to_string(),
                label: "Codex".to_string(),
                command: "codex".to_string(),
                auth_mode: "subscription".to_string(),
                credential_env: None,
            }],
        };
        let client = HttpRelayClient::new(&config).unwrap();
        client
            .publish(&SnapshotEnvelope {
                schema_version: 1,
                instance_id: config.instance_id.clone(),
                source_revision: 1,
                captured_at: 1,
                snapshot: RemoteSnapshot {
                    generated_at: 1,
                    workspaces: Vec::new(),
                    sessions: Vec::new(),
                    agents: Vec::new(),
                    activity: Vec::new(),
                    capacity: RemoteCapacity {
                        used: 0,
                        reserved: 0,
                        max: 8,
                        level: "calm".to_string(),
                        fraction: 0.0,
                        at_cap: false,
                    },
                    totals: RemoteTotals {
                        running: 0,
                        stalled: 0,
                        waiting: 0,
                        done: 0,
                        failed: 0,
                    },
                    catalog: RemoteCatalog {
                        workspaces: Vec::new(),
                        profiles: Vec::new(),
                    },
                },
            })
            .unwrap();
        let command = client.poll().unwrap().remove(0);
        assert!(matches!(command.kind, RelayCommandKind::StartRun));
        client.acknowledge(&command, "succeeded", None).unwrap();
        server.join().unwrap();

        let requests = requests.lock().unwrap();
        assert!(requests[0].starts_with("PUT /api/control-plane/desktop/snapshot HTTP/1.1"));
        assert!(requests[1].starts_with(&format!(
            "GET /api/control-plane/desktop/commands?consumer={} HTTP/1.1",
            config.consumer_id
        )));
        assert!(requests[2].starts_with(&format!(
            "PATCH /api/control-plane/desktop/commands?consumer={} HTTP/1.1",
            config.consumer_id
        )));
        for request in requests.iter() {
            let lower = request.to_ascii_lowercase();
            assert!(lower.contains("authorization: bearer desktop-token-at-least-24-characters"));
            assert!(lower.contains("x-optimus-device: primary"));
        }
        assert!(requests[2].contains(r#""status":"succeeded""#));
    }

    fn read_request(stream: &mut std::net::TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 4096];
        let mut expected = None;
        loop {
            let read = stream.read(&mut buffer).unwrap();
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read]);
            if expected.is_none() {
                if let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n")
                {
                    let headers = String::from_utf8_lossy(&bytes[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(str::trim)
                                .map(str::to_string)
                        })
                        .and_then(|value| value.parse::<usize>().ok())
                        .unwrap_or(0);
                    expected = Some(header_end + 4 + content_length);
                }
            }
            if expected.is_some_and(|length| bytes.len() >= length) {
                break;
            }
        }
        String::from_utf8(bytes).unwrap()
    }
}
