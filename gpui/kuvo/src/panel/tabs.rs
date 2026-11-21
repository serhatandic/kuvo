//! The side panel's tabs: one strip for each conversation, in the order they were opened. Every
//! tool opens as a tab here (a file, the changes, a process's output, the subagents, a side chat, a
//! terminal, a page); `+` opens the New tab page. Only where the window is: which tabs are open and
//! which shows. What's in each comes from the box.

/// What a tab shows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// The New tab page: a search for files or a page, and the tools to open.
    New,
    /// The folder's files, none open yet.
    Files,
    /// A file, by its full path on the box.
    File(String),
    /// What's changed in the folder.
    Changes,
    /// The folder's branches (opened from Changes).
    Branches,
    /// A command the conversation left running: its output as it prints.
    Process { session: String, command: String },
    /// The conversation's subagents.
    Subagents,
    /// A side chat: a side thread by the id the box gives it, or one not asked yet.
    SideChat(Option<String>),
    /// A terminal, by the tab's own number.
    Terminal(u64),
    /// A page of the computer's browser, by the tab's own number.
    Browser(u64),
}

impl Kind {
    /// Whether `self` and `other` are the same thing: opening it again shows the tab it has.
    fn same(&self, other: &Kind) -> bool {
        match (self, other) {
            // A new side chat, terminal or page is always another one.
            (Kind::SideChat(None), _) | (Kind::Terminal(_), _) | (Kind::Browser(_), _) | (Kind::New, _) => false,
            _ => self == other,
        }
    }
}

/// One tab: its number in the window, and what it shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tab {
    pub id: u64,
    pub kind: Kind,
}

/// One conversation's tabs, and the one showing. Never empty: with nothing else open, it's the
/// New tab page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Strip {
    tabs: Vec<Tab>,
    active: usize,
}

impl Strip {
    /// A strip on its New tab page.
    pub fn new(next: &mut u64) -> Strip {
        Strip { tabs: vec![Tab { id: take(next), kind: Kind::New }], active: 0 }
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    pub fn active(&self) -> &Tab {
        &self.tabs[self.active]
    }

    /// Only the New tab page is open.
    pub fn is_fresh(&self) -> bool {
        self.tabs.len() == 1 && self.tabs[0].kind == Kind::New
    }

    /// Shows `kind`: the tab it already has, or a new one in the New tab page's place when that's
    /// showing, else at the end. Answers the tab's number.
    pub fn open(&mut self, kind: Kind, next: &mut u64) -> u64 {
        if let Some(at) = self.tabs.iter().position(|tab| tab.kind.same(&kind)) {
            self.active = at;
            return self.tabs[at].id;
        }
        if self.active().kind == Kind::New {
            return self.replace_active(kind, next);
        }
        let id = take(next);
        self.tabs.push(Tab { id, kind });
        self.active = self.tabs.len() - 1;
        id
    }

    /// Opens the New tab page: the one there is, or one at the end.
    pub fn open_new(&mut self, next: &mut u64) -> u64 {
        if let Some(at) = self.tabs.iter().position(|tab| tab.kind == Kind::New) {
            self.active = at;
            return self.tabs[at].id;
        }
        let id = take(next);
        self.tabs.push(Tab { id, kind: Kind::New });
        self.active = self.tabs.len() - 1;
        id
    }

    /// Puts `kind` in the showing tab's place (a tile on the New tab page, a file picked in a
    /// Files tab), as a tab of its own; the tab it already has, if it has one, shows instead.
    pub fn replace_active(&mut self, kind: Kind, next: &mut u64) -> u64 {
        if let Some(at) = self.tabs.iter().position(|tab| tab.kind.same(&kind)) {
            if at != self.active {
                self.tabs.remove(self.active);
                self.active = if at > self.active { at - 1 } else { at };
            }
            return self.tabs[self.active].id;
        }
        let id = take(next);
        self.tabs[self.active] = Tab { id, kind };
        id
    }

    /// Shows the tab numbered `id`.
    pub fn select(&mut self, id: u64) -> bool {
        match self.tabs.iter().position(|tab| tab.id == id) {
            Some(at) => {
                self.active = at;
                true
            }
            None => false,
        }
    }

    /// Closes the tab numbered `id`: the one after it shows, else the one before; the last one
    /// closed leaves the New tab page. Answers the tab closed.
    pub fn close(&mut self, id: u64, next: &mut u64) -> Option<Tab> {
        let at = self.tabs.iter().position(|tab| tab.id == id)?;
        let closed = self.tabs.remove(at);
        if self.tabs.is_empty() {
            self.tabs.push(Tab { id: take(next), kind: Kind::New });
            self.active = 0;
        } else if at < self.active || self.active >= self.tabs.len() {
            self.active -= 1;
        }
        Some(closed)
    }

    /// Changes what the tab numbered `id` shows, in its place (a side chat once its first question
    /// starts a thread).
    pub fn set_kind(&mut self, id: u64, kind: Kind) {
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
            tab.kind = kind;
        }
    }
}

fn take(next: &mut u64) -> u64 {
    *next += 1;
    *next
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(strip: &Strip) -> Vec<Kind> {
        strip.tabs().iter().map(|tab| tab.kind.clone()).collect()
    }

    #[test]
    fn a_strip_starts_on_the_new_tab_page_and_its_first_tab_takes_its_place() {
        let mut next = 0;
        let mut strip = Strip::new(&mut next);
        assert!(strip.is_fresh());
        strip.open(Kind::Changes, &mut next);
        assert_eq!(kinds(&strip), vec![Kind::Changes]);
        strip.open(Kind::File("/p/a.ts".into()), &mut next);
        assert_eq!(kinds(&strip), vec![Kind::Changes, Kind::File("/p/a.ts".into())]);
        assert_eq!(strip.active().kind, Kind::File("/p/a.ts".into()));
    }

    #[test]
    fn opening_what_is_open_shows_its_tab_but_new_chats_terminals_and_pages_are_always_new() {
        let mut next = 0;
        let mut strip = Strip::new(&mut next);
        let changes = strip.open(Kind::Changes, &mut next);
        strip.open(Kind::File("/p/a.ts".into()), &mut next);
        assert_eq!(strip.open(Kind::Changes, &mut next), changes);
        assert_eq!(strip.active().kind, Kind::Changes);
        strip.open(Kind::Terminal(1), &mut next);
        strip.open(Kind::Terminal(2), &mut next);
        strip.open(Kind::SideChat(None), &mut next);
        strip.open(Kind::SideChat(None), &mut next);
        assert_eq!(strip.tabs().len(), 6);
    }

    #[test]
    fn closing_shows_the_next_tab_and_the_last_one_leaves_the_new_tab_page() {
        let mut next = 0;
        let mut strip = Strip::new(&mut next);
        let a = strip.open(Kind::Changes, &mut next);
        let b = strip.open(Kind::Subagents, &mut next);
        let c = strip.open(Kind::Files, &mut next);
        strip.select(b);
        strip.close(b, &mut next);
        assert_eq!(strip.active().id, c, "the one after it");
        strip.close(c, &mut next);
        assert_eq!(strip.active().id, a, "else the one before");
        strip.close(a, &mut next);
        assert!(strip.is_fresh());
    }

    #[test]
    fn a_tile_replaces_the_new_tab_page_and_shows_a_tab_already_open_instead() {
        let mut next = 0;
        let mut strip = Strip::new(&mut next);
        strip.open(Kind::Changes, &mut next);
        strip.open_new(&mut next);
        assert_eq!(kinds(&strip), vec![Kind::Changes, Kind::New]);
        strip.replace_active(Kind::Changes, &mut next);
        assert_eq!(kinds(&strip), vec![Kind::Changes]);
        strip.open_new(&mut next);
        strip.replace_active(Kind::Files, &mut next);
        assert_eq!(kinds(&strip), vec![Kind::Changes, Kind::Files]);
    }
}
