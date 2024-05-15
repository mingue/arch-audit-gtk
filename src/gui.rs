use crate::config::Config;
use crate::errors::*;
use crate::notify::{setup_inotify_thread, Event};
use crate::updater::{self, Status};
use gtk::prelude::*;
use libappindicator::{AppIndicator, AppIndicatorStatus};
use serde::{de, Deserialize, Deserializer};
use std::path::Path;
use std::str::FromStr;
use std::sync::mpsc;
use std::thread;

const CHECK_FOR_UPDATE: &str = "Check for updates";
const CHECKING: &str = "Checking...";
const QUIT: &str = "Quit";

#[derive(Debug)]
pub enum Icon {
    Check,
    Alert,
    Cross,
}

impl Icon {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Alert => "alert",
            Self::Cross => "cross",
        }
    }
}

impl FromStr for Icon {
    type Err = Error;

    fn from_str(s: &str) -> Result<Icon> {
        match s {
            "check" => Ok(Self::Check),
            "alert" => Ok(Self::Alert),
            "cross" => Ok(Self::Cross),
            _ => bail!("Invalid icon name: {:?}", s),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    s: String,
}

impl Theme {
    fn as_str(&self) -> &str {
        &self.s
    }
}

impl Default for Theme {
    fn default() -> Theme {
        Theme {
            s: "default".to_string(),
        }
    }
}

impl<'de> Deserialize<'de> for Theme {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        FromStr::from_str(&s).map_err(de::Error::custom)
    }
}

// Ensure the theme name can not be exploited for path traversal or
// other havoc. After this point icon_theme is safe to be used within
// a path.
impl FromStr for Theme {
    type Err = Error;

    fn from_str(s: &str) -> Result<Theme> {
        if s.chars().all(|c| ('a'..='z').contains(&c)) {
            Ok(Theme { s: s.to_string() })
        } else {
            bail!("Theme contains invalid characters: {:?}", s);
        }
    }
}

struct TrayIcon {
    indicator: AppIndicator,
}

impl TrayIcon {
    fn create(icon_theme: &Theme, icon: &Icon) -> Self {
        let mut indicator = AppIndicator::new("arch-audit-gtk", "");
        indicator.set_status(AppIndicatorStatus::Active);

        'outer: for path in &["./icons", "/usr/share/arch-audit-gtk/icons"] {
            for theme in &[icon_theme, &Theme::default()] {
                if let Ok(theme_path) = Path::new(path).join(theme.as_str()).canonicalize() {
                    let icon = theme_path.join("check.svg");
                    if icon.exists() {
                        indicator.set_icon_theme_path(theme_path.to_str().unwrap());
                        break 'outer;
                    }
                }
            }
        }

        indicator.set_icon_full(icon.as_str(), "icon");

        TrayIcon { indicator }
    }

    pub fn set_icon(&mut self, icon: &Icon) {
        self.indicator.set_icon_full(icon.as_str(), "icon");
    }

    pub fn add_menu(&mut self, m: &mut gtk::Menu) {
        // always append a quit item to the menu
        let mi = gtk::MenuItem::with_label(QUIT);
        m.append(&mi);
        mi.connect_activate(|_| {
            gtk::main_quit();
        });

        // set the menu
        self.indicator.set_menu(m);
        m.show_all();
    }
}

pub fn main(config: &Config) -> Result<()> {
    gtk::init()?;

    // TODO: consider a mutex and condvar so we don't queue multiple updates
    let (update_tx, update_rx) = mpsc::channel();
    let (result_tx, result_rx) = glib::MainContext::channel(glib::Priority::DEFAULT);

    setup_inotify_thread(update_tx.clone())?;

    thread::spawn(move || {
        updater::background(update_rx, result_tx);
    });

    let mut tray_icon = TrayIcon::create(&config.icon_theme, &Icon::Check);

    let mut m = gtk::Menu::new();

    let checking_mi = gtk::MenuItem::with_label(CHECK_FOR_UPDATE);
    m.append(&checking_mi);
    let mi = checking_mi.clone();
    checking_mi.connect_activate(move |_| {
        mi.set_label(CHECKING);
        update_tx.send(Event::Click).unwrap();
    });

    let status_mi = gtk::MenuItem::with_label("Starting...");
    m.append(&status_mi);

    tray_icon.add_menu(&mut m);

    result_rx.attach(None, move |msg| {
        log::info!("Received from thread: {:?}", msg);

        // update text in main menu
        checking_mi.set_label(CHECK_FOR_UPDATE);
        status_mi.set_label(&msg.text());

        match msg {
            Status::MissingUpdates(ref updates) if !updates.is_empty() => {
                let m = gtk::Menu::new();

                for update in updates {
                    let mi = gtk::MenuItem::with_label(&update.text);
                    m.append(&mi);
                    let link = update.link.to_string();
                    mi.connect_activate(move |_| {
                        if let Err(err) = opener::open(&link) {
                            eprintln!("Failed to open link: {:#}", err);
                        }
                    });
                }

                m.show_all();
                status_mi.set_submenu(Some(&m));
            }
            _ => {
                status_mi.set_submenu(None::<&gtk::Menu>);
            }
        }

        tray_icon.set_icon(&msg.icon());

        glib::ControlFlow::Continue
    });

    gtk::main();

    Ok(())
}

pub fn debug_icon(config: &Config, icon: &Icon) -> Result<()> {
    gtk::init()?;

    let mut tray_icon = TrayIcon::create(&config.icon_theme, icon);

    let mut m = gtk::Menu::new();
    tray_icon.add_menu(&mut m);

    gtk::main();

    Ok(())
}
