//! The one window: a status page while there is nothing to control, and the
//! buds' settings once the link is up. Every group shows only when the buds
//! answered for it, so another JBL model gets a window that fits it.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use adw::prelude::*;
use gtk::glib;

use crate::{
    engine::{Command, Engine, EqChoice},
    protocol::{EQ_BANDS, EQ_LIMIT, EQ_PRESETS, EqPreset, Level, Mode},
    state::{Link, State},
};

const MODES: [(Mode, &str, &str); 4] = [
    (Mode::Off, "off", "Off"),
    (Mode::Anc, "anc", "Noise Cancelling"),
    (Mode::Ambient, "ambient", "Ambient"),
    (Mode::TalkThru, "talkthru", "TalkThru"),
];

/// Index of "Custom" in the equalizer list: Off, the presets, Custom.
const EQ_CUSTOM_INDEX: u32 = EQ_PRESETS.len() as u32 + 1;

struct Gauge {
    root: gtk::Box,
    value: gtk::Label,
    bar: gtk::LevelBar,
    charging: gtk::Image,
}

impl Gauge {
    fn new(title: &str) -> Gauge {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let caption = gtk::Label::builder()
            .label(title)
            .css_classes(["caption", "dimmed"])
            .build();
        let value = gtk::Label::builder()
            .css_classes(["title-2", "numeric"])
            .build();
        let charging = gtk::Image::from_icon_name("battery-level-100-charging-symbolic");
        charging.set_tooltip_text(Some("Charging"));
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        line.set_halign(gtk::Align::Center);
        line.append(&value);
        line.append(&charging);
        let bar = gtk::LevelBar::builder().max_value(100.0).build();
        root.append(&caption);
        root.append(&line);
        root.append(&bar);
        Gauge {
            root,
            value,
            bar,
            charging,
        }
    }

    fn show(&self, level: Option<Level>) {
        match level {
            Some(l) => {
                self.value.set_label(&format!("{} %", l.percent));
                self.bar.set_value(f64::from(l.percent));
                self.root.set_tooltip_text(None);
            }
            None => {
                self.value.set_label("—");
                self.bar.set_value(0.0);
                self.root
                    .set_tooltip_text(Some("In the case, or no reading"));
            }
        }
        self.charging.set_visible(level.is_some_and(|l| l.charging));
        self.root
            .set_opacity(if level.is_some() { 1.0 } else { 0.55 });
    }
}

struct Ui {
    engine: Engine,
    /// Set while the window itself moves a control, so that does not go
    /// back out to the buds as a command.
    updating: Cell<bool>,
    title: adw::WindowTitle,
    stack: gtk::Stack,
    status: adw::StatusPage,
    toasts: adw::ToastOverlay,
    battery_group: adw::PreferencesGroup,
    gauges: [Gauge; 3],
    mode_group: adw::PreferencesGroup,
    modes: adw::ToggleGroup,
    eq_group: adw::PreferencesGroup,
    eq_choice: adw::ComboRow,
    sliders: Vec<gtk::Scale>,
    eq_timer: RefCell<Option<glib::SourceId>>,
    voice_group: adw::PreferencesGroup,
    voice_aware: adw::SpinRow,
    voice_prompts: adw::SwitchRow,
    safe_sound: adw::SwitchRow,
    auto_off: adw::SwitchRow,
    find_group: adw::PreferencesGroup,
    find_left: gtk::ToggleButton,
    find_right: gtk::ToggleButton,
    firmware: adw::ActionRow,
    notice: Cell<Option<u32>>,
}

pub fn build(
    app: &adw::Application,
    engine: Engine,
    states: async_channel::Receiver<State>,
) -> adw::ApplicationWindow {
    let ui = Rc::new(Ui::new(engine));

    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&ui.title));
    let menu = gtk::gio::Menu::new();
    menu.append(Some("About Earbuds"), Some("app.about"));
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu)
        .tooltip_text("Main Menu")
        .primary(true)
        .build();
    header.pack_end(&menu_button);

    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&ui.toasts));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Earbuds")
        .default_width(400)
        .default_height(720)
        .width_request(360)
        .height_request(294)
        .content(&view)
        .build();

    ui.connect();
    ui.render(&State::default());
    glib::spawn_future_local(glib::clone!(
        #[strong]
        ui,
        async move {
            while let Ok(mut state) = states.recv().await {
                while let Ok(newer) = states.try_recv() {
                    state = newer;
                }
                ui.render(&state);
            }
        }
    ));
    window
}

fn group(title: &str) -> adw::PreferencesGroup {
    adw::PreferencesGroup::builder().title(title).build()
}

impl Ui {
    fn new(engine: Engine) -> Ui {
        let page = adw::PreferencesPage::new();

        // Battery
        let gauges = [Gauge::new("Left"), Gauge::new("Right"), Gauge::new("Case")];
        let gauge_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .homogeneous(true)
            .spacing(18)
            .css_classes(["card", "gauges"])
            .build();
        gauges.iter().for_each(|g| gauge_row.append(&g.root));
        let battery_group = adw::PreferencesGroup::new();
        battery_group.add(&gauge_row);

        // Noise control
        let modes = adw::ToggleGroup::builder()
            .homogeneous(true)
            .css_classes(["round"])
            .build();
        for (_, name, label) in MODES {
            modes.add(adw::Toggle::builder().name(name).label(label).build());
        }
        let mode_group = group("Noise Control");
        mode_group.add(&modes);

        // Equalizer
        let mut names: Vec<&str> = vec!["Off"];
        names.extend(EQ_PRESETS.iter().map(|p| p.1));
        names.push("Custom");
        let eq_choice = adw::ComboRow::builder()
            .title("Preset")
            .model(&gtk::StringList::new(&names))
            .build();
        let bands = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .homogeneous(true)
            .css_classes(["card", "bands"])
            .build();
        let mut sliders = Vec::new();
        for freq in EQ_BANDS {
            let scale = gtk::Scale::with_range(
                gtk::Orientation::Vertical,
                -f64::from(EQ_LIMIT),
                f64::from(EQ_LIMIT),
                0.5,
            );
            scale.set_inverted(true);
            scale.set_draw_value(false);
            scale.set_height_request(150);
            scale.add_mark(0.0, gtk::PositionType::Right, None);
            scale.set_tooltip_text(Some(&format!("{} Hz", freq as u32)));
            let label = if freq >= 1000.0 {
                format!("{}k", freq as u32 / 1000)
            } else {
                format!("{}", freq as u32)
            };
            let column = gtk::Box::new(gtk::Orientation::Vertical, 4);
            column.append(&scale);
            column.append(
                &gtk::Label::builder()
                    .label(label)
                    .css_classes(["caption", "dimmed", "numeric"])
                    .build(),
            );
            bands.append(&column);
            sliders.push(scale);
        }
        let eq_group = group("Equalizer");
        eq_group.add(&eq_choice);
        eq_group.add(&bands); // not a row, so it goes below the list

        // Voice and settings
        let voice_aware = adw::SpinRow::builder()
            .title("VoiceAware")
            .subtitle("Hear your own voice on calls")
            .adjustment(&gtk::Adjustment::new(0.0, 0.0, 5.0, 1.0, 1.0, 0.0))
            .build();
        let voice_prompts = adw::SwitchRow::builder().title("Voice Prompts").build();
        let safe_sound = adw::SwitchRow::builder()
            .title("Safe Sound")
            .subtitle("Limit the volume to protect your hearing")
            .build();
        let auto_off = adw::SwitchRow::builder()
            .title("Automatic Power Off")
            .build();
        let voice_group = group("Settings");
        for row in [
            voice_aware.upcast_ref::<gtk::Widget>(),
            voice_prompts.upcast_ref(),
            safe_sound.upcast_ref(),
            auto_off.upcast_ref(),
        ] {
            voice_group.add(row);
        }

        // Find my buds
        let find_left = gtk::ToggleButton::builder()
            .label("Left")
            .valign(gtk::Align::Center)
            .build();
        let find_right = gtk::ToggleButton::builder()
            .label("Right")
            .valign(gtk::Align::Center)
            .build();
        let find_row = adw::ActionRow::builder()
            .title("Play a Sound")
            .subtitle("Loud: take them out of your ears first")
            .build();
        let buttons = gtk::Box::builder().spacing(6).build();
        buttons.append(&find_left);
        buttons.append(&find_right);
        find_row.add_suffix(&buttons);
        let find_group = group("Find My Earbuds");
        find_group.add(&find_row);

        let firmware = adw::ActionRow::builder()
            .title("Firmware")
            .css_classes(["property"])
            .build();
        let about_group = adw::PreferencesGroup::new();
        about_group.add(&firmware);

        for g in [
            &battery_group,
            &mode_group,
            &eq_group,
            &voice_group,
            &find_group,
            &about_group,
        ] {
            page.add(g);
        }

        let status = adw::StatusPage::new();
        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();
        stack.add_named(&status, Some("status"));
        stack.add_named(&page, Some("device"));
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&stack));

        Ui {
            engine,
            updating: Cell::new(false),
            title: adw::WindowTitle::new("Earbuds", ""),
            stack,
            status,
            toasts,
            battery_group,
            gauges,
            mode_group,
            modes,
            eq_group,
            eq_choice,
            sliders,
            eq_timer: RefCell::new(None),
            voice_group,
            voice_aware,
            voice_prompts,
            safe_sound,
            auto_off,
            find_group,
            find_left,
            find_right,
            firmware,
            notice: Cell::new(None),
        }
    }

    fn send(&self, command: Command) {
        if !self.updating.get() {
            self.engine.send(command);
        }
    }

    fn gains(&self) -> [f32; 10] {
        std::array::from_fn(|i| self.sliders[i].value() as f32)
    }

    fn connect(self: &Rc<Self>) {
        let ui = self.clone();
        self.modes.connect_active_name_notify(move |group| {
            let name = group.active_name();
            if let Some((mode, ..)) = MODES.iter().find(|m| Some(m.1) == name.as_deref()) {
                ui.send(Command::Mode(*mode));
            }
        });

        let ui = self.clone();
        self.eq_choice.connect_selected_notify(move |row| {
            let choice = match row.selected() {
                0 => EqChoice::Off,
                i if i == EQ_CUSTOM_INDEX => EqChoice::Custom(ui.gains()),
                i => EqChoice::Preset(EQ_PRESETS[i as usize - 1].0),
            };
            ui.send(Command::Eq(choice));
        });

        for scale in &self.sliders {
            let ui = self.clone();
            scale.connect_value_changed(move |_| {
                if ui.updating.get() {
                    return;
                }
                // Moving a band makes the curve a custom one.
                ui.updating.set(true);
                ui.eq_choice.set_selected(EQ_CUSTOM_INDEX);
                ui.updating.set(false);
                if let Some(id) = ui.eq_timer.take() {
                    id.remove();
                }
                let later = ui.clone();
                let id = glib::timeout_add_local_once(Duration::from_millis(250), move || {
                    later.eq_timer.take();
                    later.send(Command::Eq(EqChoice::Custom(later.gains())));
                });
                ui.eq_timer.replace(Some(id));
            });
        }

        let ui = self.clone();
        self.voice_aware
            .connect_value_notify(move |row| ui.send(Command::VoiceAware(row.value() as u8)));
        let ui = self.clone();
        self.voice_prompts
            .connect_active_notify(move |row| ui.send(Command::VoicePrompts(row.is_active())));
        let ui = self.clone();
        self.safe_sound
            .connect_active_notify(move |row| ui.send(Command::SafeSound(row.is_active())));
        let ui = self.clone();
        self.auto_off
            .connect_active_notify(move |row| ui.send(Command::AutoOff(row.is_active())));
        let ui = self.clone();
        self.find_left
            .connect_toggled(move |b| ui.send(Command::Find(Some(b.is_active()), None)));
        let ui = self.clone();
        self.find_right
            .connect_toggled(move |b| ui.send(Command::Find(None, Some(b.is_active()))));
    }

    fn render(&self, state: &State) {
        self.updating.set(true);
        self.render_link(state);
        if state.link == Link::Ready {
            self.render_device(state);
        }
        self.updating.set(false);

        if let Some((n, message)) = &state.notice
            && self.notice.replace(Some(*n)) != Some(*n)
        {
            self.toasts.add_toast(adw::Toast::new(message));
        }
    }

    fn render_link(&self, state: &State) {
        let name = state.name.as_deref().unwrap_or("JBL earbuds");
        let (subtitle, icon, title, description) = match state.link {
            Link::NoBluetooth => (
                "",
                Some("bluetooth-disabled-symbolic"),
                "Bluetooth Is Off",
                "Turn Bluetooth on to use your earbuds",
            ),
            Link::NoDevice => (
                "",
                Some("audio-headphones-symbolic"),
                "No Earbuds Connected",
                "Connect your JBL earbuds in the Bluetooth settings",
            ),
            Link::Searching | Link::Connecting => (
                name,
                None,
                "Connecting…",
                "Take an earbud out of the case if this takes long",
            ),
            Link::Ready => (name, None, "", ""),
        };
        self.title.set_subtitle(subtitle);
        if state.link == Link::Ready {
            self.stack.set_visible_child_name("device");
            return;
        }
        self.stack.set_visible_child_name("status");
        self.status.set_title(title);
        self.status.set_description(Some(description));
        match icon {
            Some(icon) => {
                self.status.set_paintable(gtk::gdk::Paintable::NONE);
                self.status.set_icon_name(Some(icon));
            }
            None if self.status.paintable().is_none() => {
                self.status.set_icon_name(None);
                self.status
                    .set_paintable(Some(&adw::SpinnerPaintable::new(Some(&self.status))));
            }
            None => {}
        }
    }

    fn render_device(&self, s: &State) {
        let b = s.battery;
        self.gauges[0].show(b.left);
        self.gauges[1].show(b.right);
        self.gauges[2].show(b.case);
        self.battery_group
            .set_visible(b.left.is_some() || b.right.is_some() || b.case.is_some());

        self.mode_group.set_visible(s.mode.is_some());
        // One bud in the case: the buds decline noise cancelling, and quietly.
        let one_in_case = b.left.is_some() != b.right.is_some();
        self.mode_group.set_description(
            one_in_case.then_some("Noise cancelling needs both earbuds out of the case"),
        );
        for (mode, name, _) in MODES {
            if let Some(toggle) = self.modes.toggle_by_name(name) {
                toggle.set_enabled(s.modes.contains(&mode));
            }
        }
        if let Some(mode) = s.mode {
            let name = MODES.iter().find(|m| m.0 == mode).map(|m| m.1);
            self.modes.set_active_name(name);
        }

        self.eq_group.set_visible(s.eq.is_some());
        if let Some(eq) = s.eq {
            let selected = match eq.preset {
                EqPreset::Off => 0,
                EqPreset::Custom => EQ_CUSTOM_INDEX,
                EqPreset::Preset(c) => EQ_PRESETS
                    .iter()
                    .position(|p| p.0 == c)
                    .map_or(EQ_CUSTOM_INDEX, |i| i as u32 + 1),
            };
            self.eq_choice.set_selected(selected);
            // Leave the sliders alone while a drag is still on its way out.
            if self.eq_timer.borrow().is_none() {
                for (scale, gain) in self.sliders.iter().zip(eq.gains) {
                    scale.set_value(gain.into());
                }
            }
        }

        if let Some(level) = s.voice_aware {
            self.voice_aware.set_value(level.into());
        }
        self.voice_aware.set_visible(s.voice_aware.is_some());
        set_switch(&self.voice_prompts, s.voice_prompts);
        set_switch(&self.safe_sound, s.safe_sound);
        set_switch(&self.auto_off, s.auto_off.map(|a| a.enabled));
        self.auto_off.set_subtitle(&match s.auto_off {
            Some(a) if a.minutes > 0 => format!("After {} minutes without sound", a.minutes),
            _ => "When not in use for a while".to_owned(),
        });
        self.voice_group.set_visible(
            s.voice_aware.is_some()
                || s.voice_prompts.is_some()
                || s.safe_sound.is_some()
                || s.auto_off.is_some(),
        );

        self.find_group.set_visible(s.find.is_some());
        if let Some((left, right)) = s.find {
            self.find_left.set_active(left);
            self.find_right.set_active(right);
        }

        self.firmware
            .set_subtitle(s.firmware.as_deref().unwrap_or("—"));
        self.firmware.set_visible(s.firmware.is_some());
    }
}

fn set_switch(row: &adw::SwitchRow, value: Option<bool>) {
    row.set_visible(value.is_some());
    if let Some(on) = value {
        row.set_active(on);
    }
}
