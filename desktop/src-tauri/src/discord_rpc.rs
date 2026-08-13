use discord_rich_presence::{activity, DiscordIpc, DiscordIpcClient};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Default)]
pub struct DiscordRpc {
    inner: Arc<Mutex<DiscordRpcInner>>,
}

#[derive(Default)]
struct DiscordRpcInner {
    enabled: bool,
    app_id: Option<String>,
    client: Option<DiscordIpcClient>,
    started_at: i64,
}

impl DiscordRpc {
    pub fn configure(&self, enabled: bool, app_id: Option<String>) {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let app_id = app_id.filter(|value| !value.trim().is_empty());
        if inner.app_id != app_id || !enabled {
            if let Some(client) = inner.client.as_mut() {
                let _ = client.close();
            }
            inner.client = None;
        }
        inner.enabled = enabled;
        inner.app_id = app_id;
        if inner.started_at == 0 {
            inner.started_at = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs() as i64)
                .unwrap_or_default();
        }
        if enabled {
            Self::set_activity(&mut inner, "В лаунчере", "Выбирает профиль");
        }
    }

    pub fn browsing(&self, profile: Option<&str>, version: Option<&str>) {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let details = profile
            .map(|name| format!("Профиль: {name}"))
            .unwrap_or_else(|| "Выбирает профиль".to_string());
        let state = version
            .map(|value| format!("Minecraft {value}"))
            .unwrap_or_else(|| "В лаунчере".to_string());
        Self::set_activity(&mut inner, &state, &details);
    }

    pub fn preparing(&self, profile: &str, version: &str) {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        Self::set_activity(
            &mut inner,
            &format!("Minecraft {version}"),
            &format!("Подготавливает {profile}"),
        );
    }

    pub fn playing(&self, profile: &str, version: &str) {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        Self::set_activity(
            &mut inner,
            &format!("Minecraft {version}"),
            &format!("Играет: {profile}"),
        );
    }

    fn set_activity(inner: &mut DiscordRpcInner, state: &str, details: &str) {
        if !inner.enabled {
            return;
        }
        if inner.client.is_none() {
            let Some(app_id) = inner.app_id.as_deref() else {
                return;
            };
            let mut client = DiscordIpcClient::new(app_id);
            if client.connect().is_err() {
                return;
            }
            inner.client = Some(client);
        }
        let activity = activity::Activity::new()
            .details(details)
            .state(state)
            .timestamps(activity::Timestamps::new().start(inner.started_at));
        if inner
            .client
            .as_mut()
            .is_some_and(|client| client.set_activity(activity).is_err())
        {
            inner.client = None;
        }
    }
}

impl Drop for DiscordRpcInner {
    fn drop(&mut self) {
        if let Some(client) = self.client.as_mut() {
            let _ = client.clear_activity();
            let _ = client.close();
        }
    }
}
