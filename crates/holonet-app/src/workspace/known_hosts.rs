//! Known hosts (KnownHosts.dc.html): trusted server keys with search,
//! filters, a detail panel, copy, forget, and review of a recorded mismatch.
//! Import and export of `known_hosts` files (milestone 7) are in the toolbar.

use gpui::{
    AnyElement, ClipboardItem, Context, Div, FontWeight, IntoElement, SharedString, Window, div, prelude::*, px,
};
use holonet_model::{Host, KnownHost, KnownHostSource, VaultData, key_type_label};

use super::hosts::format_date;
use super::{KhDialog, Page, Workspace};
use crate::theme::{self, TypeStyles};
use crate::ui::{Button, ButtonKind, callout, dialog, dialog_buttons, icon, key_values, modal, section_label};

const ASIDE: f32 = 320.;
const FILTERS: [&str; 4] = ["All", "Saved hosts", "No saved host", "Needs review"];

/// One row: a trusted key and the saved host that uses it, if any.
struct Row<'a> {
    known: &'a KnownHost,
    host: Option<&'a Host>,
    fingerprint: String,
}

impl Row<'_> {
    fn name(&self) -> String {
        match self.host {
            Some(h) => h.display_name().to_string(),
            // An imported hashed name can be matched but not read.
            None if self.known.is_hashed() => "Hashed name".into(),
            None => self.known.host.clone(),
        }
    }

    fn address(&self) -> String {
        match (self.known.is_hashed(), self.host) {
            (true, Some(h)) => format!("{}:{} (hashed in known_hosts)", h.address.trim(), h.port),
            (true, None) => "Hashed in known_hosts; no saved host matches it".into(),
            _ => format!("{}:{}", self.known.host, self.known.port),
        }
    }

    fn status(&self) -> (&'static str, gpui::Rgba) {
        if self.known.mismatch.is_some() {
            ("Mismatch seen", theme::warn())
        } else if self.host.is_some() {
            ("Trusted", theme::accent())
        } else {
            ("Trusted, no host", theme::ink_muted())
        }
    }

    fn matches(&self, query: &str, filter: usize) -> bool {
        let pass = match filter {
            1 => self.host.is_some(),
            2 => self.host.is_none(),
            3 => self.known.mismatch.is_some(),
            _ => true,
        };
        if !pass {
            return false;
        }
        if query.is_empty() {
            return true;
        }
        let q = query.to_lowercase();
        [self.name(), self.address(), self.fingerprint.clone(), self.known.key_type_label().to_string()]
            .iter()
            .any(|s| s.to_lowercase().contains(&q))
    }
}

fn rows(data: &VaultData) -> Vec<Row<'_>> {
    let mut rows: Vec<Row> = data
        .known_hosts
        .iter()
        .map(|k| Row {
            known: k,
            host: if k.is_hashed() {
                // A hashed name is found by trying each saved host's addresses.
                data.hosts.iter().find(|h| {
                    k.matches(h.address.trim(), h.port) || h.tailscale().is_some_and(|t| k.matches(t, h.port))
                })
            } else {
                data.hosts_for(&k.host, k.port).next()
            },
            fingerprint: holonet_ssh::fingerprint(&k.public_key).unwrap_or_default(),
        })
        .collect();
    rows.sort_by_key(|r| r.name().to_lowercase());
    rows
}

impl Workspace {
    pub(super) fn known_hosts_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(data) = self.vault.as_ref().map(|v| v.data().clone()) else {
            return div().into_any_element();
        };
        let all = rows(&data);
        let query = self.kh_search.read(cx).text().trim().to_string();
        let shown: Vec<&Row> = all.iter().filter(|r| r.matches(&query, self.kh_filter)).collect();
        if self.kh_selected.as_ref().is_none_or(|(h, p)| !shown.iter().any(|r| r.known.matches(h, *p))) {
            self.kh_selected = shown.first().map(|r| (r.known.host.clone(), r.known.port));
        }
        let selected = self.kh_selected.clone();

        let header = div()
            .flex()
            .flex_col()
            .gap(theme::SPACE_1)
            .child(div().text_title().text_color(theme::ink()).child("Known hosts"))
            .child(div().text_body().text_color(theme::ink_muted()).child(
                "Server keys you have trusted. Holonet stops a connection if a server presents a different one.",
            ));

        let pills = FILTERS.iter().enumerate().map(|(i, label)| {
            let on = i == self.kh_filter;
            div()
                .id(SharedString::from(format!("kh-filter-{i}")))
                .h(theme::CONTROL)
                .px(px(14.))
                .flex()
                .items_center()
                .rounded(px(22.))
                .border_1()
                .text_body()
                .text_color(theme::ink())
                .cursor_pointer()
                .when(on, |d| d.bg(theme::raised()).border_color(theme::accent()))
                .when(!on, |d| d.border_color(theme::line()).hover(|s| s.bg(theme::card())))
                .child(*label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.kh_filter = i;
                    cx.notify();
                }))
        });
        let toolbar =
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(theme::SPACE_2)
                .child(div().w(px(340.)).child(self.kh_search.clone()))
                .children(pills)
                .child(div().flex_1())
                .child(Button::secondary("kh-import", "Import known_hosts").on_click(
                    cx.listener(|this, _, _, cx| this.open_import(super::import::ImportKind::KnownHosts, cx)),
                ))
                .when(!data.known_hosts.is_empty(), |d| {
                    d.child(
                        Button::secondary("kh-export", "Export known_hosts")
                            .on_click(cx.listener(|this, _, _, cx| this.export_known_hosts(cx))),
                    )
                });

        let body: AnyElement = if data.known_hosts.is_empty() {
            div()
                .py(theme::SPACE_6)
                .flex()
                .flex_col()
                .items_center()
                .gap(theme::SPACE_3)
                .child(icon("known-hosts", px(40.), theme::ink_faint()))
                .child(div().text_heading().text_color(theme::ink()).child("No trusted keys yet"))
                .child(
                    div()
                        .text_body()
                        .text_color(theme::ink_muted())
                        .child("Keys appear here when you choose Trust and connect on a first connection."),
                )
                .into_any_element()
        } else if shown.is_empty() {
            div()
                .py(theme::SPACE_5)
                .text_body()
                .text_color(theme::ink_muted())
                .child("No known hosts match this filter.")
                .into_any_element()
        } else {
            self.kh_table(&shown, selected.as_ref(), cx).into_any_element()
        };

        let detail = selected
            .as_ref()
            .and_then(|(h, p)| all.iter().find(|r| r.known.matches(h, *p)))
            .map(|row| self.kh_detail(row, cx));
        let dialog_layer = self.kh_dialog_layer(&all, cx);

        div()
            .size_full()
            .relative()
            .flex()
            .child(
                div().id("kh-scroll").flex_1().min_w_0().h_full().overflow_y_scroll().child(
                    div()
                        .px(theme::SPACE_6)
                        .py(px(28.))
                        .flex()
                        .flex_col()
                        .gap(px(18.))
                        .child(header)
                        .child(toolbar)
                        .child(body),
                ),
            )
            .children(detail)
            .children(dialog_layer)
            .into_any_element()
    }

    fn kh_table(&self, rows: &[&Row], selected: Option<&(String, u16)>, cx: &mut Context<Self>) -> Div {
        let columns = |d: Div| d.grid().grid_cols(12).gap(theme::SPACE_3).items_center().px(theme::SPACE_4);
        let header = columns(div().py(px(10.)))
            .text_label()
            .text_color(theme::ink_muted())
            .child(div().col_span(3).child("HOST"))
            .child(div().col_span(2).child("KEY TYPE"))
            .child(div().col_span(3).child("FINGERPRINT"))
            .child(div().col_span(2).child("TRUSTED ON"))
            .child(div().col_span(2).child("STATUS"));
        let lines = rows.iter().map(|row| {
            let key = (row.known.host.clone(), row.known.port);
            let on = selected == Some(&key);
            let (status, color) = row.status();
            columns(div())
                .id(SharedString::from(format!("kh-{}:{}", key.0, key.1)))
                .py(theme::SPACE_3)
                .border_t_1()
                .border_color(theme::line())
                .cursor_pointer()
                .when(on, |d| d.bg(theme::card_selected()))
                .when(!on, |d| d.hover(|s| s.bg(theme::raised())))
                .child(
                    div()
                        .col_span(3)
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(theme::SPACE_2)
                        .child(icon("role-server", px(16.), theme::no_group()))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .min_w_0()
                                .child(
                                    div()
                                        .text_body()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(theme::ink())
                                        .truncate()
                                        .child(SharedString::from(row.name())),
                                )
                                .child(
                                    div()
                                        .text_mono_small()
                                        .text_color(theme::ink_muted())
                                        .truncate()
                                        .child(SharedString::from(row.address())),
                                ),
                        ),
                )
                .child(
                    div().col_span(2).text_mono_small().text_color(theme::ink_soft()).child(row.known.key_type_label()),
                )
                .child(
                    div()
                        .col_span(3)
                        .min_w_0()
                        .text_mono_small()
                        .text_color(theme::ink_soft())
                        .truncate()
                        .child(SharedString::from(row.fingerprint.clone())),
                )
                .child(
                    div()
                        .col_span(2)
                        .text_meta()
                        .text_color(theme::ink_muted())
                        .child(SharedString::from(format_date(row.known.trusted_at))),
                )
                .child(div().col_span(2).text_meta().text_color(color).child(status))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.kh_selected = Some(key.clone());
                    cx.notify();
                }))
        });
        div().bg(theme::card()).rounded(theme::RADIUS_LG).min_w(px(640.)).child(header).children(lines)
    }

    fn kh_detail(&self, row: &Row, cx: &mut Context<Self>) -> Div {
        let fingerprint = row.fingerprint.clone();
        let public_key = row.known.public_key.clone();
        let source = match row.known.source {
            KnownHostSource::FirstConnection => "First connection",
            KnownHostSource::ImportedKnownHosts => "Imported known_hosts",
        };
        let last_matched: SharedString =
            row.known.last_matched_at.map(format_date).unwrap_or_else(|| "Not yet".into()).into();
        let saved_host: AnyElement = match row.host {
            Some(h) => {
                let id = h.id;
                div()
                    .id("kh-saved-host")
                    .text_color(theme::accent())
                    .cursor_pointer()
                    .child(SharedString::from(h.display_name().to_string()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = Some(id);
                        this.page = Page::Hosts;
                        cx.notify();
                    }))
                    .into_any_element()
            }
            None => "None".into_any_element(),
        };
        div()
            .w(px(ASIDE))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .gap(px(18.))
            .px(theme::SPACE_5)
            .py(px(28.))
            .bg(theme::panel())
            .shadow(theme::shadow_panel())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme::SPACE_1)
                    .child(div().text_heading().text_color(theme::ink()).child(SharedString::from(row.name())))
                    .child(
                        div().text_mono_small().text_color(theme::ink_muted()).child(SharedString::from(row.address())),
                    ),
            )
            .when(row.known.mismatch.is_some(), |d| {
                d.child(
                    callout(crate::ui::CalloutKind::Warning)
                        .child(
                            div()
                                .text_color(theme::warn())
                                .font_weight(FontWeight::MEDIUM)
                                .child("A different key was presented"),
                        )
                        .child(
                            div().text_meta().child(
                                "The last connection attempt was stopped. The key below is still the trusted one.",
                            ),
                        )
                        .child(
                            div().pt(theme::SPACE_2).child(
                                Button::new("review-mismatch", "Review mismatch", ButtonKind::DestructiveFill)
                                    .compact()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.kh_dialog = Some(KhDialog::Review { expected: false });
                                        cx.notify();
                                    })),
                            ),
                        ),
                )
            })
            .child(
                div().flex().flex_col().gap(theme::SPACE_2).child(section_label("Trusted fingerprint")).child(
                    div()
                        .p(theme::SPACE_3)
                        .bg(theme::sunken())
                        .rounded(theme::RADIUS_MD)
                        .text_mono_small()
                        .text_color(theme::ink_soft())
                        .child(SharedString::from(format!("{} {}", row.known.key_type_label(), row.fingerprint))),
                ),
            )
            .child(key_values(vec![
                ("Trusted on".into(), SharedString::from(format_date(row.known.trusted_at)).into_any_element()),
                ("Last matched".into(), last_matched.into_any_element()),
                ("Source".into(), source.into_any_element()),
                ("Saved host".into(), saved_host),
            ]))
            // Stacked: two buttons do not fit side by side in the 320px panel.
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme::SPACE_2)
                    .child(div().child(Button::secondary("copy-fp", "Copy fingerprint").full_width().on_click(
                        move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(fingerprint.clone())),
                    )))
                    .child(div().child(Button::secondary("copy-pk", "Copy public key").full_width().on_click(
                        move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(public_key.clone())),
                    ))),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme::SPACE_2)
                    .child(
                        Button::new("forget-key", "Forget this key", ButtonKind::DestructiveOutline)
                            .full_width()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.kh_dialog = Some(KhDialog::Forget);
                                cx.notify();
                            })),
                    )
                    .child(div().text_meta().text_color(theme::ink_muted()).child(
                        "After forgetting, Holonet asks you to verify the fingerprint again on the next connection.",
                    )),
            )
    }

    fn kh_dialog_layer(&self, rows: &[Row], cx: &mut Context<Self>) -> Option<AnyElement> {
        let which = self.kh_dialog?;
        let (host, port) = self.kh_selected.clone()?;
        let row = rows.iter().find(|r| r.known.matches(&host, port))?;
        let name = row.name();
        let entity = cx.entity();
        let close = {
            let entity = entity.clone();
            move |_: &mut Window, cx: &mut gpui::App| {
                entity.update(cx, |w, cx| {
                    w.kh_dialog = None;
                    cx.notify()
                })
            }
        };
        let card = match which {
            KhDialog::Forget => {
                let (h, p) = (host.clone(), port);
                dialog(
                    true,
                    format!("Forget the key for {name}?"),
                    "Holonet will ask you to verify this server's fingerprint again the next time you connect.",
                )
                .child(dialog_buttons([
                    Button::secondary("forget-cancel", "Cancel").on_click({
                        let close = close.clone();
                        move |_, w, cx| close(w, cx)
                    }),
                    Button::new("forget-confirm", "Forget key", ButtonKind::DestructiveFill).on_click(cx.listener(
                        move |this, _, _, cx| {
                            let (h, p) = (h.clone(), p);
                            this.save(
                                move |d| {
                                    d.forget_host_key(&h, p);
                                },
                                cx,
                            );
                            this.kh_dialog = None;
                            this.kh_selected = None;
                        },
                    )),
                ]))
                .into_any_element()
            }
            KhDialog::Review { expected } => {
                let mismatch = row.known.mismatch.clone()?;
                let presented_fp = holonet_ssh::fingerprint(&mismatch.presented_key).unwrap_or_default();
                let presented_type = key_type_label(mismatch.presented_key.split_whitespace().next().unwrap_or(""));
                let fp_card = |label: &str, fp: String, meta: String, warn: bool| {
                    div()
                        .flex_1()
                        .min_w(px(180.))
                        .flex()
                        .flex_col()
                        .gap(theme::SPACE_1)
                        .p(px(14.))
                        .bg(theme::ground())
                        .border_1()
                        .border_color(if warn { theme::warn() } else { theme::line() })
                        .rounded(theme::RADIUS_MD)
                        .child(
                            div()
                                .text_meta()
                                .text_color(if warn { theme::warn() } else { theme::ink_muted() })
                                .child(SharedString::from(label.to_string())),
                        )
                        .child(div().text_mono_small().text_color(theme::ink()).child(SharedString::from(fp)))
                        .child(div().text_meta().text_color(theme::ink_muted()).child(SharedString::from(meta)))
                };
                let (h1, p1) = (host.clone(), port);
                let (h2, p2, presented) = (host.clone(), port, mismatch.presented_key.clone());
                dialog(
                    true,
                    format!("The host key for {name} changed"),
                    "Holonet stopped the connection and did not send your credentials. Nothing connects from here.",
                )
                .max_w(px(600.))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(10.))
                        .child(fp_card(
                            "Trusted in Known hosts",
                            row.fingerprint.clone(),
                            format!("Saved {}", format_date(row.known.trusted_at)),
                            false,
                        ))
                        .child(fp_card(
                            "Presented",
                            presented_fp,
                            format!("{presented_type} · seen {}", format_date(mismatch.seen_at)),
                            true,
                        )),
                )
                .child(
                    div()
                        .id("kh-expected")
                        .flex()
                        .items_center()
                        .gap(theme::SPACE_2)
                        .cursor_pointer()
                        .text_body()
                        .text_color(theme::warn_ink())
                        .child(icon(if expected { "chevron-down" } else { "chevron-up" }, px(14.), theme::warn_ink()))
                        .child("I expected this change")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.kh_dialog = Some(KhDialog::Review { expected: !expected });
                            cx.notify();
                        })),
                )
                .when(expected, |d| {
                    d.child(div().text_body().text_color(theme::warn_ink()).child(
                        "Only replace it if you have confirmed the new fingerprint with whoever runs the server.",
                    ))
                    .child(div().flex().child(
                        Button::new("kh-replace", "Replace trusted key", ButtonKind::DestructiveOutline).on_click(
                            cx.listener(move |this, _, _, cx| {
                                let (h, p, k) = (h2.clone(), p2, presented.clone());
                                this.save(move |d| d.trust_host_key(&h, p, &k, KnownHostSource::FirstConnection), cx);
                                this.kh_dialog = None;
                            }),
                        ),
                    ))
                })
                .child(dialog_buttons([
                    Button::secondary("kh-close", "Close").on_click({
                        let close = close.clone();
                        move |_, w, cx| close(w, cx)
                    }),
                    Button::primary("kh-keep", "Keep the trusted key").on_click(cx.listener(move |this, _, _, cx| {
                        let (h, p) = (h1.clone(), p1);
                        this.save(
                            move |d| {
                                if let Some(k) = d.known_host_mut(&h, p) {
                                    k.mismatch = None;
                                }
                            },
                            cx,
                        );
                        this.kh_dialog = None;
                    })),
                ]))
                .into_any_element()
            }
        };
        Some(modal("kh-dialog", px(600.), card, close).into_any_element())
    }
}
