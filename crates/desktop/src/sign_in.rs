use std::rc::Rc;

use gpui::{App, ClickEvent, Entity, PathPromptOptions, Window, div, prelude::*, px};
use gpui_m3::{Button, Dialog, LoadingIndicator, Theme};

use crate::account::{Account, AccountState};

/// Extension that exports a site's cookies as a Netscape `cookies.txt`, for Chrome and Edge.
const COOKIES_EXTENSION_URL: &str = "https://chromewebstore.google.com/detail/get-cookiestxt-locally/cclelndahbckbenkjhflpdbgdldlbecc";

/// The fallback when Google refuses the sign-in window: export the session by hand.
const MANUAL_STEPS: [&str; 3] = [
    "En una ventana de incógnito, inicia sesión en music.youtube.com.",
    "Exporta las cookies con “Get cookies.txt LOCALLY” y cierra la ventana.",
    "Elige el archivo aquí; después puedes borrarlo.",
];

/// Signs in with Google in a window of the app, or with an exported `cookies.txt`.
#[derive(IntoElement)]
pub struct SignInDialog {
    account: Entity<Account>,
    on_close: CloseHandler,
}

type CloseHandler = Rc<dyn Fn(&mut Window, &mut App)>;

impl SignInDialog {
    pub fn new(
        account: Entity<Account>,
        on_close: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        SignInDialog {
            account,
            on_close: Rc::new(on_close),
        }
    }
}

impl RenderOnce for SignInDialog {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let s = *Theme::of(cx);
        let account = self.account.read(cx);
        let signing_in = matches!(account.state(), AccountState::SigningIn);
        let error = account.error().cloned();

        let manual_steps = MANUAL_STEPS.iter().enumerate().map(|(index, step)| {
            div()
                .text_sm()
                .text_color(s.on_surface_variant)
                .child(format!("{}. {step}", index + 1))
        });

        let google_account = self.account.clone();
        let file_account = self.account.clone();
        let close = self.on_close.clone();
        let dismiss = self.on_close.clone();
        Dialog::new("sign-in")
            .title("Iniciar sesión en YouTube Music")
            .width(px(480.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(div().text_sm().text_color(s.on_surface_variant).child(
                        "Se abrirá una ventana de Google. Inicia sesión con tu cuenta de \
                         YouTube Music Premium y se cerrará sola.",
                    ))
                    .child(if signing_in {
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(LoadingIndicator::new().size(px(32.)))
                            .child("Completa el inicio de sesión en la ventana de Google…")
                            .into_any_element()
                    } else {
                        Button::new("sign-in-google", "Continuar con Google")
                            .filled()
                            .on_click(move |_: &ClickEvent, _, cx| {
                                google_account
                                    .update(cx, |account, cx| account.sign_in_with_google(cx));
                            })
                            .into_any_element()
                    })
                    .children(error.map(|error| div().text_sm().text_color(s.error).child(error)))
                    .child(div().h(px(1.)).bg(s.outline_variant))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .child("¿Google no te deja entrar? Importa la sesión:"),
                            )
                            .children(manual_steps),
                    ),
            )
            .action(
                Button::new("sign-in-extension", "Ver extensión")
                    .text()
                    .on_click(|_: &ClickEvent, _, cx| cx.open_url(COOKIES_EXTENSION_URL)),
            )
            .action(
                Button::new("sign-in-pick", "Importar cookies.txt…")
                    .text()
                    .disabled(signing_in)
                    .on_click(move |_: &ClickEvent, _, cx| {
                        let paths = cx.prompt_for_paths(PathPromptOptions {
                            files: true,
                            directories: false,
                            multiple: false,
                            prompt: Some("Importar sesión".into()),
                        });
                        let account = file_account.clone();
                        cx.spawn(async move |cx| {
                            let Ok(Ok(Some(paths))) = paths.await else {
                                return;
                            };
                            let Some(path) = paths.into_iter().next() else {
                                return;
                            };
                            account
                                .update(cx, |account, cx| account.sign_in_with_file(path, cx))
                                .ok();
                        })
                        .detach();
                    }),
            )
            .action(
                Button::new("sign-in-cancel", "Cancelar")
                    .text()
                    .on_click(move |_: &ClickEvent, window, cx| close(window, cx)),
            )
            .on_dismiss(move |_: &ClickEvent, window, cx| dismiss(window, cx))
    }
}
