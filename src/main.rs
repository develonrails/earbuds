//! Earbuds: settings for JBL earbuds, in a small native GNOME window.

mod att;
mod bluez;
mod engine;
mod protocol;
mod state;
mod window;

use adw::prelude::*;
use gtk::{gio, glib};

pub const APP_ID: &str = "io.github.develonrails.Earbuds";

const CSS: &str = "
.gauges { padding: 14px 18px; }
.bands { padding: 12px 6px 8px 6px; }
";

fn about(app: &adw::Application) {
    let dialog = adw::AboutDialog::builder()
        .application_name("Earbuds")
        .application_icon(APP_ID)
        .developer_name("Mitchell de Jong")
        .version(env!("CARGO_PKG_VERSION"))
        .comments("Settings for JBL earbuds. Not affiliated with JBL or Harman.")
        .website("https://github.com/develonrails/earbuds")
        .issue_url("https://github.com/develonrails/earbuds/issues")
        .license_type(gtk::License::MitX11)
        .build();
    dialog.add_credit_section(
        Some("Protocol"),
        &["jbl-control by Ahmad Elyan https://github.com/AhmadRun/jbl-control"],
    );
    dialog.present(app.active_window().as_ref());
}

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();

    app.connect_startup(|app| {
        let css = gtk::CssProvider::new();
        css.load_from_string(CSS);
        gtk::style_context_add_provider_for_display(
            &gtk::gdk::Display::default().expect("display"),
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        let about_action = gio::SimpleAction::new("about", None);
        about_action.connect_activate(glib::clone!(
            #[weak]
            app,
            move |_, _| about(&app)
        ));
        app.add_action(&about_action);
        let quit = gio::SimpleAction::new("quit", None);
        quit.connect_activate(glib::clone!(
            #[weak]
            app,
            move |_, _| app.quit()
        ));
        app.add_action(&quit);
        app.set_accels_for_action("app.quit", &["<Control>q"]);
        app.set_accels_for_action("window.close", &["<Control>w"]);
    });

    app.connect_activate(|app| {
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        let (engine, states) = engine::start();
        window::build(app, engine, states).present();
    });

    app.run()
}
