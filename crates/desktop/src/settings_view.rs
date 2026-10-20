use gpui::{
    ClickEvent, Context, Entity, EventEmitter, SharedString, Subscription, Window, div, prelude::*,
    px,
};
use gpui_m3::{Button, KeyCombo, KeyNames, ListGroup, SettingRow, Slider, Switch, Theme};
use ytm_core::Client;

use crate::account::Account;
use crate::settings;
use crate::ui::section_title;

/// The longest crossfade offered, in seconds.
const MAX_CROSSFADE_SECS: f32 = 12.;

pub enum SettingsEvent {
    SetDark(bool),
    SignIn,
}

/// The size of the audio cache, measured when the page opens and after it is emptied.
enum CacheSize {
    Measuring,
    Known(u64),
    Failed,
}

/// Playback, appearance, storage and account settings.
pub struct SettingsView {
    client: Client,
    account: Entity<Account>,
    cache_size: CacheSize,
    clearing: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SettingsEvent> for SettingsView {}

impl SettingsView {
    pub fn new(client: Client, account: Entity<Account>, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe(&account, |_, _, cx| cx.notify())];
        SettingsView {
            client,
            account,
            cache_size: CacheSize::Measuring,
            clearing: false,
            _subscriptions: subscriptions,
        }
    }

    /// Measures the audio cache again; the app calls it each time the page opens.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.cache_size = CacheSize::Measuring;
        cx.notify();
        let size = self.client.audio_cache_bytes();
        cx.spawn(async move |this, cx| {
            let size = size.await;
            this.update(cx, |this, cx| {
                this.cache_size = match size {
                    Ok(bytes) => CacheSize::Known(bytes),
                    Err(err) => {
                        log::warn!("could not measure the audio cache: {err:#}");
                        CacheSize::Failed
                    }
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn clear_cache(&mut self, cx: &mut Context<Self>) {
        if self.clearing {
            return;
        }
        self.clearing = true;
        cx.notify();
        let clear = self.client.clear_audio_cache();
        cx.spawn(async move |this, cx| {
            let result = clear.await;
            this.update(cx, |this, cx| {
                this.clearing = false;
                if let Err(err) = result {
                    log::warn!("could not empty the audio cache: {err:#}");
                }
                this.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    fn render_playback(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let saved = settings::get(cx);
        let normalize = saved.normalize.unwrap_or(true);
        let crossfade = saved.crossfade_secs.clamp(0., MAX_CROSSFADE_SECS);
        let view = cx.entity().downgrade();
        let view_for_slider = view.clone();

        ListGroup::new()
            .title("Reproducción")
            .custom_row(
                SettingRow::new("normalize", "Normalizar volumen")
                    .description("Iguala el volumen entre canciones")
                    .control(Switch::new("normalize-switch", normalize).on_toggle(
                        move |on, _, cx| {
                            settings::update(cx, |settings| settings.normalize = Some(on));
                            view.update(cx, |_, cx| cx.notify()).ok();
                        },
                    )),
            )
            .custom_row(
                SettingRow::new("crossfade", "Fundido entre canciones")
                    .description(crossfade_label(crossfade))
                    .control(
                        Slider::new("crossfade-slider", crossfade, 0.0..=MAX_CROSSFADE_SECS)
                            .step(1.)
                            .width(px(200.))
                            .on_change(move |secs, _, cx| {
                                let secs = secs.round().clamp(0., MAX_CROSSFADE_SECS);
                                // Dragging reports every move: save only real changes.
                                if settings::get(cx).crossfade_secs != secs {
                                    settings::update(cx, |settings| settings.crossfade_secs = secs);
                                    view_for_slider.update(cx, |_, cx| cx.notify()).ok();
                                }
                            }),
                    ),
            )
    }

    fn render_appearance(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let dark = Theme::get(cx).dark;
        let view = cx.entity().downgrade();
        ListGroup::new().title("Apariencia").custom_row(
            SettingRow::new("dark", "Tema oscuro")
                .description("Los colores siguen la portada de lo que suena")
                .control(
                    Switch::new("dark-switch", dark).on_toggle(move |on, _, cx| {
                        view.update(cx, |_, cx| cx.emit(SettingsEvent::SetDark(on)))
                            .ok();
                    }),
                ),
        )
    }

    fn render_storage(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let size: SharedString = match self.cache_size {
            CacheSize::Measuring => "Calculando…".into(),
            CacheSize::Known(bytes) => format_size(bytes).into(),
            CacheSize::Failed => "No se pudo calcular".into(),
        };
        let empty = matches!(self.cache_size, CacheSize::Known(0));
        ListGroup::new().title("Almacenamiento").custom_row(
            SettingRow::new("audio-cache", "Caché de audio")
                .description(format!(
                    "{size} · Canciones guardadas para que vuelvan a sonar sin esperar"
                ))
                .control(
                    Button::new(
                        "clear-cache",
                        if self.clearing {
                            "Vaciando…"
                        } else {
                            "Vaciar caché"
                        },
                    )
                    .tonal()
                    .disabled(self.clearing || empty)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.clear_cache(cx))),
                ),
        )
    }

    fn render_account(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let signed_in = self.account.read(cx).is_signed_in();
        let row = if signed_in {
            let account = self.account.clone();
            SettingRow::new("session", "Sesión de YouTube Music")
                .description("Tus playlists, me gusta e historial vienen de tu cuenta")
                .control(
                    Button::new("sign-out", "Cerrar sesión")
                        .outlined()
                        .on_click(move |_: &ClickEvent, _, cx| {
                            account.update(cx, |account, cx| account.sign_out(cx));
                        }),
                )
        } else {
            SettingRow::new("session", "Sin sesión")
                .description("Inicia sesión para ver tu biblioteca y dar me gusta")
                .control(Button::new("sign-in", "Iniciar sesión").filled().on_click(
                    cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(SettingsEvent::SignIn)),
                ))
        };
        ListGroup::new().title("Cuenta").custom_row(row)
    }
}

/// The keyboard shortcuts (see `shortcuts.rs`), as the settings list them.
const SHORTCUTS: [(&str, &[&str]); 10] = [
    ("Reproducir o pausar", &["space"]),
    ("Retroceder o avanzar 5 s", &["left", "right"]),
    ("Bajar o subir el volumen", &["down", "up"]),
    ("Canción anterior o siguiente", &["ctrl-left", "ctrl-right"]),
    ("Silenciar", &["m"]),
    ("Aleatorio y repetir", &["s", "r"]),
    ("Buscar", &["ctrl-f", "/"]),
    ("Volver atrás", &["alt-left", "backspace"]),
    ("Cerrar menú o Reproduciendo", &["escape"]),
    ("Mini reproductor", &["ctrl-shift-m"]),
];

fn render_shortcuts() -> impl IntoElement {
    SHORTCUTS.iter().enumerate().fold(
        ListGroup::new().title("Atajos de teclado"),
        |group, (index, (action, keys))| {
            let combos = keys.iter().enumerate().map(|(key_index, key)| {
                KeyCombo::new(("shortcut", index * 4 + key_index), *key).names(KeyNames::spanish())
            });
            group.row(*action, div().flex().gap_2().children(combos))
        },
    )
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = *Theme::of(cx);
        div()
            .id("settings")
            .size_full()
            .overflow_y_scroll()
            .px_4()
            .pb_6()
            .child(
                div()
                    .max_w(px(720.))
                    .flex()
                    .flex_col()
                    .gap_6()
                    .child(section_title("Ajustes"))
                    .child(self.render_playback(cx))
                    .child(self.render_appearance(cx))
                    .child(self.render_storage(cx))
                    .child(self.render_account(cx))
                    .child(render_shortcuts())
                    .child(
                        div()
                            .px(px(6.))
                            .text_sm()
                            .text_color(s.on_surface_variant)
                            .child(format!("{} {}", crate::APP_NAME, env!("CARGO_PKG_VERSION"))),
                    ),
            )
    }
}

fn crossfade_label(secs: f32) -> String {
    if secs < 0.5 {
        "Sin pausas: cada canción empieza justo al terminar la anterior".to_string()
    } else {
        format!("{secs:.0} s de fundido al cambiar de canción")
    }
}

/// 1536 → "1,5 KB", as the rest of the app writes decimals.
fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024. && unit + 1 < UNITS.len() {
        value /= 1024.;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit]).replace('.', ",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_like_the_rest_of_the_app() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1536), "1,5 KB");
        assert_eq!(format_size(300 * 1024 * 1024), "300,0 MB");
        assert_eq!(format_size(3 * 1024 * 1024 * 1024), "3,0 GB");
    }
}
