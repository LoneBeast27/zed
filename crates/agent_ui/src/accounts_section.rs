//! Connected-account status section for the settings status panel.

use std::sync::Arc;

use gpui::{AnyElement, App, FontWeight, Hsla, SharedString};
use http_client::HttpClient;
use serde::Deserialize;
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::prelude::*;

use crate::agent_accents::{STATUS_IDLE, STATUS_RUNNING};
use crate::bridge::{self, BRIDGE_BASE_URL};
use crate::task_board::style::{SURFACE_1, tabular_nums};

#[derive(Debug, Clone, PartialEq)]
pub struct ConnectedAccounts {
    rows: [AccountRow; 3],
}

#[derive(Debug, Clone, PartialEq)]
pub struct AccountRow {
    vendor: Vendor,
    connected: bool,
    method: String,
    detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Vendor {
    Claude,
    Codex,
    Google,
}

#[derive(Debug, Default, Deserialize)]
struct AccountsResponse {
    #[serde(default)]
    claude: RawAccountStatus,
    #[serde(default)]
    codex: RawAccountStatus,
    #[serde(default)]
    google: RawAccountStatus,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawAccountStatus {
    #[serde(default)]
    connected: bool,
    #[serde(default)]
    method: String,
    #[serde(default)]
    detail: String,
    #[serde(default, rename = "last_refresh_s")]
    _last_refresh_s: Option<f64>,
}

pub async fn fetch_accounts(http: Arc<dyn HttpClient>) -> ConnectedAccounts {
    let url = format!("{BRIDGE_BASE_URL}/accounts");
    bridge::fetch_json(http.as_ref(), &url)
        .await
        .map(|raw| ConnectedAccounts::from_json(&raw))
        .unwrap_or_else(|_| ConnectedAccounts::unavailable())
}

pub async fn connect_claude_and_fetch(http: Arc<dyn HttpClient>) -> ConnectedAccounts {
    let url = format!("{BRIDGE_BASE_URL}/accounts/claude/connect");
    let _ = bridge::post_json(http.as_ref(), &url, "{}".to_string()).await;
    fetch_accounts(http).await
}

impl ConnectedAccounts {
    pub fn from_json(raw: &str) -> Self {
        serde_json::from_str::<AccountsResponse>(raw)
            .map(Self::from_response)
            .unwrap_or_else(|_| Self::unavailable())
    }

    pub fn unavailable() -> Self {
        Self {
            rows: [
                AccountRow::unavailable(Vendor::Claude),
                AccountRow::unavailable(Vendor::Codex),
                AccountRow::unavailable(Vendor::Google),
            ],
        }
    }

    fn from_response(response: AccountsResponse) -> Self {
        Self {
            rows: [
                AccountRow::from_status(Vendor::Claude, response.claude),
                AccountRow::from_status(Vendor::Codex, response.codex),
                AccountRow::from_status(Vendor::Google, response.google),
            ],
        }
    }
}

impl AccountRow {
    fn from_status(vendor: Vendor, status: RawAccountStatus) -> Self {
        let method = if status.method.is_empty() {
            vendor.default_method().to_string()
        } else {
            status.method
        };
        let detail = if status.detail.is_empty() {
            "status unavailable".to_string()
        } else {
            status.detail
        };
        Self {
            vendor,
            connected: status.connected,
            method,
            detail,
        }
    }

    fn unavailable(vendor: Vendor) -> Self {
        Self {
            vendor,
            connected: false,
            method: vendor.default_method().to_string(),
            detail: "status unavailable".to_string(),
        }
    }
}

impl Vendor {
    fn label(self) -> &'static str {
        match self {
            Vendor::Claude => "Claude",
            Vendor::Codex => "Codex",
            Vendor::Google => "Google",
        }
    }

    fn default_method(self) -> &'static str {
        match self {
            Vendor::Claude => "session-key",
            Vendor::Codex => "run-telemetry",
            Vendor::Google => "local-api",
        }
    }
}

pub fn render_connected_accounts(
    accounts: &ConnectedAccounts,
    claude_connect: Button,
    cx: &App,
) -> AnyElement {
    let colors = cx.theme().colors();
    v_flex()
        .w_full()
        .max_w(px(720.))
        .rounded(px(12.))
        .bg(SURFACE_1)
        .border_1()
        .border_color(colors.border)
        .px(px(18.))
        .py(px(16.))
        .mb(px(16.))
        .child(
            div()
                .mb(px(12.))
                .text_size(px(15.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.text)
                .child("Connected accounts"),
        )
        .children({
            let mut claude_connect = Some(claude_connect);
            accounts.rows.iter().map(move |row| {
                let button = (row.vendor == Vendor::Claude)
                    .then(|| claude_connect.take())
                    .flatten();
                render_account_row(row, button, cx)
            })
        })
        .into_any_element()
}

fn render_account_row(row: &AccountRow, button: Option<Button>, cx: &App) -> AnyElement {
    let colors = cx.theme().colors();
    let mono = ThemeSettings::get_global(cx).buffer_font.family.clone();
    let dot_color: Hsla = if row.connected {
        STATUS_RUNNING.into()
    } else {
        STATUS_IDLE.into()
    };
    h_flex()
        .w_full()
        .items_center()
        .gap(px(12.))
        .py(px(7.))
        .child(div().size(px(7.)).rounded_full().bg(dot_color).flex_none())
        .child(
            div()
                .w(px(64.))
                .flex_none()
                .text_size(px(14.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.text)
                .child(row.vendor.label()),
        )
        .child(method_chip(&row.method, &mono, cx))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .text_size(px(12.))
                .text_color(colors.text_placeholder)
                .truncate()
                .child(SharedString::from(row.detail.clone())),
        )
        .child(match button {
            Some(button) => button.into_any_element(),
            None => div()
                .flex_none()
                .text_size(px(12.))
                .font_family(mono)
                .font_features(tabular_nums())
                .text_color(colors.text_muted)
                .child(SharedString::from(row.method.clone()))
                .into_any_element(),
        })
        .into_any_element()
}

fn method_chip(method: &str, mono: &SharedString, cx: &App) -> AnyElement {
    let colors = cx.theme().colors();
    h_flex()
        .flex_none()
        .items_center()
        .px(px(8.))
        .py(px(3.))
        .rounded(px(7.))
        .bg(colors.element_background)
        .child(
            div()
                .font_family(mono.clone())
                .font_features(tabular_nums())
                .text_size(px(12.))
                .text_color(colors.text_muted)
                .child(SharedString::from(method.to_string())),
        )
        .into_any_element()
}
