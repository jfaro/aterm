//! Prefix and Resize mode keybindings.

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;

use super::input;

pub const RESIZE_STEP_SML: i16 = 2;
pub const RESIZE_STEP_LRG: i16 = 8;

pub enum PrefixAction {
    Literal,
    NewShell,
    NewCommand,
    NewFolder,
    Rename,
    FocusNext,
    FocusPrev,
    FocusIndex,
    MoveDown,
    MoveUp,
    CloseOrDelete,
    ResizeLeft,
    ResizeRight,
    ToggleSidebar,
    ToggleSidebarPosition,
    Detach,
    ShowHelp,
}

pub struct PrefixBinding {
    pub matches: fn(&KeyEvent) -> bool,
    /// Shown in the compact status-bar hint bar; `None` for help-only bindings.
    pub hint: Option<&'static str>,
    pub help_keys: &'static str,
    pub help_desc: &'static str,
    pub action: PrefixAction,
}

fn is_shift(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::SHIFT)
}

pub const PREFIX_BINDINGS: &[PrefixBinding] = &[
    PrefixBinding {
        matches: input::is_prefix,
        hint: None,
        help_keys: "ctrl-b",
        help_desc: "send a literal ctrl-b",
        action: PrefixAction::Literal,
    },
    PrefixBinding {
        matches: |key| key.code == KeyCode::Char('c'),
        hint: Some("c shell"),
        help_keys: "c",
        help_desc: "new shell pane in the current folder",
        action: PrefixAction::NewShell,
    },
    PrefixBinding {
        matches: |key| key.code == KeyCode::Char('a'),
        hint: Some("a command"),
        help_keys: "a",
        help_desc: "new pane running a command",
        action: PrefixAction::NewCommand,
    },
    PrefixBinding {
        matches: |key| key.code == KeyCode::Char('f'),
        hint: Some("f folder"),
        help_keys: "f",
        help_desc: "new folder",
        action: PrefixAction::NewFolder,
    },
    PrefixBinding {
        matches: |key| key.code == KeyCode::Char('r'),
        hint: Some("r rename"),
        help_keys: "r",
        help_desc: "rename pane or folder",
        action: PrefixAction::Rename,
    },
    PrefixBinding {
        matches: |key| {
            key.code == KeyCode::Char('J') || (key.code == KeyCode::Down && is_shift(key))
        },
        hint: Some("J/K move"),
        help_keys: "J / shift-↓",
        help_desc: "move pane down, across folders",
        action: PrefixAction::MoveDown,
    },
    PrefixBinding {
        matches: |key| key.code == KeyCode::Char('K') || (key.code == KeyCode::Up && is_shift(key)),
        hint: None,
        help_keys: "K / shift-↑",
        help_desc: "move pane up, across folders",
        action: PrefixAction::MoveUp,
    },
    PrefixBinding {
        matches: |key| {
            matches!(key.code, KeyCode::Char('n' | 'j'))
                || (key.code == KeyCode::Down && !is_shift(key))
        },
        hint: Some("n/p switch"),
        help_keys: "n / j / ↓",
        help_desc: "next pane or folder",
        action: PrefixAction::FocusNext,
    },
    PrefixBinding {
        matches: |key| {
            matches!(key.code, KeyCode::Char('p' | 'k'))
                || (key.code == KeyCode::Up && !is_shift(key))
        },
        hint: None,
        help_keys: "p / k / ↑",
        help_desc: "previous pane or folder",
        action: PrefixAction::FocusPrev,
    },
    PrefixBinding {
        matches: |key| matches!(key.code, KeyCode::Char('1'..='9')),
        hint: None,
        help_keys: "1–9",
        help_desc: "focus pane by position",
        action: PrefixAction::FocusIndex,
    },
    PrefixBinding {
        matches: |key| key.code == KeyCode::Char('x'),
        hint: Some("x close"),
        help_keys: "x",
        help_desc: "close pane / delete folder (keeps panes)",
        action: PrefixAction::CloseOrDelete,
    },
    PrefixBinding {
        matches: |key| key.code == KeyCode::Char('<'),
        hint: Some("</> resize"),
        help_keys: "< / >",
        help_desc: "resize sidebar",
        action: PrefixAction::ResizeLeft,
    },
    PrefixBinding {
        matches: |key| key.code == KeyCode::Char('>'),
        hint: None,
        help_keys: "",
        help_desc: "",
        action: PrefixAction::ResizeRight,
    },
    PrefixBinding {
        matches: |key| key.code == KeyCode::Char('b'),
        hint: Some("b sidebar"),
        help_keys: "b",
        help_desc: "hide/show sidebar",
        action: PrefixAction::ToggleSidebar,
    },
    PrefixBinding {
        matches: |key| key.code == KeyCode::Char('B'),
        hint: None,
        help_keys: "B",
        help_desc: "move sidebar to bottom/left",
        action: PrefixAction::ToggleSidebarPosition,
    },
    PrefixBinding {
        matches: |key| matches!(key.code, KeyCode::Char('d' | 'q')),
        hint: Some("d detach"),
        help_keys: "d / q",
        help_desc: "detach",
        action: PrefixAction::Detach,
    },
    PrefixBinding {
        matches: |key| key.code == KeyCode::Char('?'),
        hint: Some("? help"),
        help_keys: "?",
        help_desc: "show this help",
        action: PrefixAction::ShowHelp,
    },
];

pub enum ResizeAction {
    Adjust(i16),
    Reset,
}

pub struct ResizeBinding {
    pub matches: fn(&KeyEvent) -> bool,
    pub hint: Option<&'static str>,
    pub help_keys: &'static str,
    pub help_desc: &'static str,
    pub action: ResizeAction,
}

pub const RESIZE_BINDINGS: &[ResizeBinding] = &[
    ResizeBinding {
        matches: |key| {
            matches!(key.code, KeyCode::Char('<' | 'h'))
                || (key.code == KeyCode::Left && !is_shift(key))
        },
        hint: Some("←/→ adjust"),
        help_keys: "← / → / h / l",
        help_desc: "adjust by 2",
        action: ResizeAction::Adjust(-RESIZE_STEP_SML),
    },
    ResizeBinding {
        matches: |key| {
            matches!(key.code, KeyCode::Char('>' | 'l'))
                || (key.code == KeyCode::Right && !is_shift(key))
        },
        hint: None,
        help_keys: "",
        help_desc: "",
        action: ResizeAction::Adjust(RESIZE_STEP_SML),
    },
    ResizeBinding {
        matches: |key| {
            key.code == KeyCode::Char('H') || (key.code == KeyCode::Left && is_shift(key))
        },
        hint: Some("shift ±8"),
        help_keys: "shift / H / L",
        help_desc: "adjust by 8",
        action: ResizeAction::Adjust(-RESIZE_STEP_LRG),
    },
    ResizeBinding {
        matches: |key| {
            key.code == KeyCode::Char('L') || (key.code == KeyCode::Right && is_shift(key))
        },
        hint: None,
        help_keys: "",
        help_desc: "",
        action: ResizeAction::Adjust(RESIZE_STEP_LRG),
    },
    ResizeBinding {
        matches: |key| key.code == KeyCode::Char('='),
        hint: Some("= reset"),
        help_keys: "=",
        help_desc: "reset width",
        action: ResizeAction::Reset,
    },
];
