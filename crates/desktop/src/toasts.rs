//! Short messages at the bottom of the window confirming an action or explaining a failure. One
//! shows at a time; a new one replaces it.

use std::time::Duration;

use gpui::{App, AppContext, Context, Entity, Global, SharedString, Task};

/// How long a message stays on screen.
const VISIBLE_FOR: Duration = Duration::from_secs(4);

pub struct Toasts {
    /// The message on screen, with a number that changes for every message so each one plays
    /// its entrance.
    current: Option<(u64, SharedString)>,
    shown: u64,
    _hide: Option<Task<()>>,
}

struct GlobalToasts(Entity<Toasts>);

impl Global for GlobalToasts {}

pub fn init(cx: &mut App) {
    let toasts = cx.new(|_| Toasts {
        current: None,
        shown: 0,
        _hide: None,
    });
    cx.set_global(GlobalToasts(toasts));
}

/// Shows `text` for a few seconds.
pub fn show(text: impl Into<SharedString>, cx: &mut App) {
    let text = text.into();
    Toasts::global(cx).update(cx, |toasts, cx| toasts.show(text, cx));
}

impl Toasts {
    /// The app's messages; the window observes them.
    pub fn global(cx: &App) -> Entity<Toasts> {
        cx.global::<GlobalToasts>().0.clone()
    }

    pub fn current(&self) -> Option<&(u64, SharedString)> {
        self.current.as_ref()
    }

    fn show(&mut self, text: SharedString, cx: &mut Context<Self>) {
        self.shown += 1;
        let shown = self.shown;
        self.current = Some((shown, text));
        self._hide = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(VISIBLE_FOR).await;
            this.update(cx, |this, cx| {
                if this.current.as_ref().is_some_and(|(id, _)| *id == shown) {
                    this.current = None;
                    cx.notify();
                }
            })
            .ok();
        }));
        cx.notify();
    }
}
