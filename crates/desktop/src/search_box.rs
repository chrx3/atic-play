//! The search field and what it suggests while the user types: completed searches, artists,
//! songs and albums from YouTube Music, and the latest searches while it is empty.

use std::time::Duration;

use gpui::{
    App, Context, Corner, Entity, EventEmitter, SharedString, Subscription, Task, Window, div,
    prelude::*, relative,
};
use gpui_m3::{
    Suggestion, SuggestionAccept, SuggestionDismiss, SuggestionList, SuggestionNav, SuggestionNext,
    SuggestionPrevious, TextField, TextFieldEvent,
};
use ytm_core::{Album, Artist, Client, SearchSuggestions, Track};

use crate::assets;
use crate::navigation::Navigation;
use crate::playback::Playback;
use crate::settings;

/// Typing pauses this long before suggestions are asked for, so each keystroke is not a request.
const DEBOUNCE: Duration = Duration::from_millis(150);
const MAX_RECENT_SEARCHES: usize = 8;
const MAX_TERMS: usize = 5;
const LIST_WIDTH: f32 = 520.;

pub enum SearchBoxEvent {
    Search(String),
}

/// What a suggestion does when picked.
#[derive(Clone)]
enum Pick {
    Search(String),
    Track(Track),
    Album(Album),
    Artist(Artist),
}

pub struct SearchBox {
    client: Client,
    playback: Entity<Playback>,
    navigation: Entity<Navigation>,
    field: Entity<TextField>,
    /// The suggestions on offer, and their rows in the list.
    picks: Vec<Pick>,
    rows: Vec<Suggestion>,
    /// The text the suggestions are for; its match is highlighted in each row.
    query: SharedString,
    nav: SuggestionNav,
    /// Closed by Escape, a search or a pick, until the text changes again.
    dismissed: bool,
    /// Text the box put in the field itself, whose change must not reopen the list.
    applied: Option<SharedString>,
    /// Bumped on every change, so slow suggestions for older text are dropped.
    generation: u64,
    /// The pending request; replacing it cancels the previous one.
    request: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SearchBoxEvent> for SearchBox {}

impl SearchBox {
    pub fn new(
        client: Client,
        playback: Entity<Playback>,
        navigation: Entity<Navigation>,
        cx: &mut Context<Self>,
    ) -> Self {
        let field = cx.new(|cx| {
            TextField::new(cx)
                .placeholder("¿Qué quieres escuchar?")
                .icon("search")
        });
        let subscriptions =
            vec![
                cx.subscribe(&field, |this, _, event: &TextFieldEvent, cx| match event {
                    TextFieldEvent::Changed(text) => this.text_changed(text.clone(), cx),
                    TextFieldEvent::Submitted(text) => this.search(text.to_string(), cx),
                    TextFieldEvent::Cancelled => this.dismiss(cx),
                }),
            ];
        let mut search_box = SearchBox {
            client,
            playback,
            navigation,
            field,
            picks: Vec::new(),
            rows: Vec::new(),
            query: SharedString::default(),
            nav: SuggestionNav::default(),
            dismissed: false,
            applied: None,
            generation: 0,
            request: None,
            _subscriptions: subscriptions,
        };
        search_box.show_recent_searches(cx);
        search_box
    }

    fn text_changed(&mut self, text: SharedString, cx: &mut Context<Self>) {
        if self.applied.take().as_ref() == Some(&text) {
            return;
        }
        self.dismissed = false;
        self.generation += 1;
        let query = text.trim().to_string();
        if query.is_empty() {
            self.request = None;
            self.show_recent_searches(cx);
            return;
        }

        // Searching for exactly what was typed is always the first row, right away; the rest
        // follow when YouTube Music answers.
        let mut picks = vec![Pick::Search(query.clone())];
        picks.extend(
            self.picks
                .iter()
                .filter(|pick| !matches!(pick, Pick::Search(_)))
                .cloned(),
        );
        self.set_picks(picks, query.clone().into(), cx);

        let generation = self.generation;
        let suggestions = self.client.search_suggestions(query.clone());
        self.request = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            let suggestions = suggestions.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match suggestions {
                    Ok(suggestions) => {
                        this.set_picks(picks_for(&query, suggestions), query.into(), cx)
                    }
                    Err(err) => log::warn!("could not load search suggestions: {err:#}"),
                }
            })
            .ok();
        }));
    }

    fn show_recent_searches(&mut self, cx: &mut Context<Self>) {
        let picks = settings::get(cx)
            .recent_searches
            .iter()
            .cloned()
            .map(Pick::Search)
            .collect();
        self.set_picks(picks, SharedString::default(), cx);
    }

    fn set_picks(&mut self, picks: Vec<Pick>, query: SharedString, cx: &mut Context<Self>) {
        let recent = query.is_empty();
        self.rows = picks.iter().map(|pick| row(pick, recent)).collect();
        self.picks = picks;
        self.query = query;
        self.nav.reset(self.picks.len());
        cx.notify();
    }

    /// Searches for `query`, remembering it among the recent searches.
    fn search(&mut self, query: String, cx: &mut Context<Self>) {
        let query = query.trim().to_string();
        if query.is_empty() {
            return;
        }
        self.dismiss(cx);
        settings::update(cx, |settings| {
            let recent = &mut settings.recent_searches;
            recent.retain(|earlier| !earlier.eq_ignore_ascii_case(&query));
            recent.insert(0, query.clone());
            recent.truncate(MAX_RECENT_SEARCHES);
        });
        if self.field.read(cx).text().as_ref() != query {
            let text: SharedString = query.clone().into();
            self.applied = Some(text.clone());
            self.field.update(cx, |field, cx| field.set_text(text, cx));
        }
        cx.emit(SearchBoxEvent::Search(query));
    }

    fn accept(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(pick) = self.picks.get(index).cloned() else {
            return;
        };
        match pick {
            Pick::Search(query) => self.search(query, cx),
            Pick::Track(track) => {
                self.dismiss(cx);
                self.playback
                    .update(cx, |playback, cx| playback.play_tracks(vec![track], 0, cx));
            }
            Pick::Album(album) => {
                self.dismiss(cx);
                self.navigation.update(cx, |navigation, cx| {
                    navigation.open_album(album.id.clone(), Some(album), cx)
                });
            }
            Pick::Artist(artist) => {
                self.dismiss(cx);
                self.navigation.update(cx, |navigation, cx| {
                    navigation.open_artist(artist.id.clone(), Some(artist), cx)
                });
            }
        }
    }

    /// Puts the cursor in the search field (Ctrl+F).
    pub fn focus(&self, window: &mut Window, cx: &App) {
        self.field.read(cx).focus(window);
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        if !self.dismissed {
            self.dismissed = true;
            cx.notify();
        }
    }
}

impl Render for SearchBox {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let open =
            !self.dismissed && !self.rows.is_empty() && self.field.read(cx).is_focused(window);
        // While the list is open, ↑ ↓ Enter Tab Esc work it instead of the field.
        if self.field.read(cx).is_suggesting() != open {
            self.field
                .update(cx, |field, cx| field.set_suggesting(open, cx));
        }
        let list = open.then(|| {
            let hover = cx.entity().downgrade();
            let pick = hover.clone();
            div().absolute().left_0().top(relative(1.)).child(
                SuggestionList::new("search-suggestions", self.rows.clone())
                    .query(self.query.clone())
                    .selected(self.nav.selected())
                    .width(LIST_WIDTH)
                    .anchor(Corner::TopLeft)
                    .on_hover(move |row, _, cx| {
                        hover
                            .update(cx, |this, cx| {
                                if this.nav.hover(row) {
                                    cx.notify();
                                }
                            })
                            .ok();
                    })
                    .on_pick(move |row, _, cx| {
                        pick.update(cx, |this, cx| this.accept(row, cx)).ok();
                    }),
            )
        });
        div()
            .relative()
            .w_full()
            .on_action(cx.listener(|this, _: &SuggestionNext, _, cx| {
                this.nav.next();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SuggestionPrevious, _, cx| {
                this.nav.previous();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SuggestionAccept, _, cx| {
                if let Some(index) = this.nav.current() {
                    this.accept(index, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &SuggestionDismiss, _, cx| this.dismiss(cx)))
            .child(self.field.clone())
            .children(list)
    }
}

/// The list for `query`: the query itself, then completed searches, artists, songs and albums.
fn picks_for(query: &str, suggestions: SearchSuggestions) -> Vec<Pick> {
    let mut picks = vec![Pick::Search(query.to_string())];
    picks.extend(
        suggestions
            .terms
            .into_iter()
            .filter(|term| !term.trim().eq_ignore_ascii_case(query))
            .take(MAX_TERMS)
            .map(Pick::Search),
    );
    picks.extend(suggestions.artists.into_iter().map(Pick::Artist));
    picks.extend(suggestions.tracks.into_iter().map(Pick::Track));
    picks.extend(suggestions.albums.into_iter().map(Pick::Album));
    picks
}

fn row(pick: &Pick, recent: bool) -> Suggestion {
    match pick {
        Pick::Search(query) => {
            Suggestion::new(if recent { "history" } else { "search" }, query.clone())
        }
        Pick::Artist(artist) => {
            Suggestion::new(assets::PERSON, artist.name.clone()).hint("Artista")
        }
        Pick::Track(track) => Suggestion::new(assets::MUSIC, track.title.clone())
            .hint(format!("Canción · {}", track.artists)),
        Pick::Album(album) => Suggestion::new(assets::ALBUM, album.title.clone())
            .hint(format!("Álbum · {}", album.artists)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_query_comes_first_and_is_not_repeated() {
        let suggestions = SearchSuggestions {
            terms: vec!["Bad Bunny".into(), "bad bunny antiguas".into()],
            ..SearchSuggestions::default()
        };
        let picks = picks_for("bad bunny", suggestions);
        let queries: Vec<_> = picks
            .iter()
            .map(|pick| match pick {
                Pick::Search(query) => query.as_str(),
                _ => "",
            })
            .collect();
        assert_eq!(queries, ["bad bunny", "bad bunny antiguas"]);
    }
}
