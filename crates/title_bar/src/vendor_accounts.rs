use futures::AsyncReadExt as _;
use gpui::http_client::{AsyncBody, HttpClient, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use ui::prelude::*;
use ui::{Color, Indicator, Label, LabelSize};

const BRIDGE_URL: &str = "http://127.0.0.1:4530";

#[derive(Clone, Debug, Default, Deserialize)]
pub struct VendorAccounts {
    #[serde(default)]
    pub claude: VendorAccount,
    #[serde(default)]
    pub codex: VendorAccount,
    #[serde(default)]
    pub google: VendorAccount,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct VendorAccount {
    #[serde(default)]
    pub connected: bool,
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub detail: String,
}

impl VendorAccounts {
    fn unavailable() -> Self {
        let unavailable = VendorAccount {
            connected: false,
            method: String::new(),
            detail: "unavailable".to_string(),
        };

        Self {
            claude: unavailable.clone(),
            codex: unavailable.clone(),
            google: unavailable,
        }
    }
}

pub async fn fetch(http: Arc<dyn HttpClient>) -> VendorAccounts {
    let Ok(mut response) = http
        .get(&format!("{BRIDGE_URL}/accounts"), AsyncBody::empty(), true)
        .await
    else {
        return VendorAccounts::unavailable();
    };

    let mut body = Vec::new();
    if response.body_mut().read_to_end(&mut body).await.is_err() {
        return VendorAccounts::unavailable();
    }

    serde_json::from_slice(&body).unwrap_or_else(|_| VendorAccounts::unavailable())
}

pub async fn connect_claude(http: Arc<dyn HttpClient>) {
    #[derive(Serialize)]
    struct Empty {}

    let _ = http
        .post_json(
            &format!("{BRIDGE_URL}/accounts/claude/connect"),
            Json(Empty {}).into(),
        )
        .await;
}

pub fn render_popover(
    accounts: &VendorAccounts,
    on_connect: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    v_flex()
        .w_64()
        .p_2()
        .gap_1()
        .child(Label::new("Vendor accounts").size(LabelSize::Small))
        .child(render_account_row(
            "Claude",
            &accounts.claude,
            Some(Button::new("connect-claude", "Connect")
                .label_size(LabelSize::XSmall)
                .size(ButtonSize::Compact)
                .on_click(on_connect)),
        ))
        .child(render_account_row("Codex", &accounts.codex, passive_note(&accounts.codex)))
        .child(render_account_row(
            "Google",
            &accounts.google,
            passive_note(&accounts.google),
        ))
}

fn render_account_row(
    name: &'static str,
    account: &VendorAccount,
    end_slot: Option<impl IntoElement>,
) -> impl IntoElement {
    h_flex()
        .min_h_8()
        .w_full()
        .gap_2()
        .justify_between()
        .child(
            h_flex()
                .min_w_0()
                .gap_2()
                .child(
                    Indicator::dot().color(if account.connected {
                        Color::Success
                    } else {
                        Color::Muted
                    }),
                )
                .child(
                    v_flex()
                        .min_w_0()
                        .child(Label::new(name).size(LabelSize::Small))
                        .child(
                            Label::new(if account.detail.is_empty() {
                                "Not connected"
                            } else {
                                &account.detail
                            })
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                        ),
                ),
        )
        .when_some(end_slot, |this, end_slot| this.child(end_slot))
}

fn passive_note(account: &VendorAccount) -> Option<impl IntoElement> {
    (!account.method.is_empty()).then(|| {
        Label::new(account.method.clone())
            .size(LabelSize::XSmall)
            .color(Color::Muted)
    })
}
