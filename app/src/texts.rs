use crate::ui::Status;

pub struct Texts {
    ready: &'static str,
    active: &'static str,
    needs_permission: &'static str,
    pair: &'static str,
    pub allow_accessibility: &'static str,
    pub allow_input_monitoring: &'static str,
    pub offline: &'static str,
    pub connecting: &'static str,
    pub searching: &'static str,
    pub layout: &'static str,
    pub wifi_fix: &'static str,
    pub wifi_fix_prompt: &'static str,
    pub quit: &'static str,
}

impl Texts {
    pub fn status(&self, status: Status) -> &'static str {
        match status {
            Status::NeedsPermission => self.needs_permission,
            Status::Ready => self.ready,
            Status::Active => self.active,
        }
    }

    pub fn pair(&self, name: &str) -> String {
        self.pair.replace("{}", name)
    }
}

const FR: Texts = Texts {
    ready: "Prêt",
    active: "Actif, tenir Ctrl+Alt+Échap 1 s pour revenir",
    needs_permission: "Autorisations macOS requises",
    pair: "Associer {}…",
    allow_accessibility: "Autoriser : Accessibilité…",
    allow_input_monitoring: "Autoriser : Surveillance de l'entrée…",
    offline: "hors ligne",
    connecting: "connexion…",
    searching: "Recherche d'appareils…",
    layout: "Disposition des écrans…",
    wifi_fix: "Réduire les saccades Wi-Fi…",
    wifi_fix_prompt: "Jon Mouse's veut installer un petit service qui coupe AirDrop et Continuité pendant qu'il tourne. Sur Mac, ils font saccader le curseur en Wi-Fi. Ils reviennent dès que tu quittes l'app.",
    quit: "Quitter Jon Mouse's",
};

const EN: Texts = Texts {
    ready: "Ready",
    active: "Active, hold Ctrl+Alt+Esc for 1 s to come back",
    needs_permission: "macOS permissions required",
    pair: "Pair {}…",
    allow_accessibility: "Allow: Accessibility…",
    allow_input_monitoring: "Allow: Input Monitoring…",
    offline: "offline",
    connecting: "connecting…",
    searching: "Looking for devices…",
    layout: "Screen layout…",
    wifi_fix: "Reduce Wi-Fi stutter…",
    wifi_fix_prompt: "Jon Mouse's wants to install a small service that turns AirDrop and Continuity off while it runs. On a Mac they make the cursor stutter over Wi-Fi. They come back as soon as you quit the app.",
    quit: "Quit Jon Mouse's",
};

pub fn get() -> &'static Texts {
    match sys_locale::get_locale() {
        Some(locale) if locale.starts_with("fr") => &FR,
        _ => &EN,
    }
}
