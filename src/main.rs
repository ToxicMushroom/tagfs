use clap::Parser;
use fuser::MountOption;
use gtk4::ffi::{gtk_entry_grab_focus_without_selecting, gtk_text_grab_focus_without_selecting, gtk_widget_grab_focus};
use gtk4::gdk::{Key, ModifierType};
use gtk4::glib::{Object, Propagation, Properties};
use gtk4::prelude::{ActionMapExt, BoxExt, ButtonExt, Cast, CastNone, GtkWindowExt, ListItemExt, WidgetExt};
use gtk4::{gio, glib, Button, CallbackAction, ColumnView, FilterListModel, GridView, KeyvalTrigger, Label, ListBox, ListItem, ListView, MenuButton, ScrolledWindow, SearchBar, SearchEntry, Shortcut, ShortcutAction, ShortcutScope, ShortcutTrigger, SignalListItemFactory, SingleSelection, Video};
use libadwaita::prelude::{ApplicationExt, ApplicationExtManual};
use libadwaita::{gtk, Application, ApplicationWindow};
use log::{error, LevelFilter};
use pretty_env_logger::env_logger::Builder;
use std::cell::Cell;
use std::fs::File;

use crate::file::TagNumber;
use crate::fs::backing::ExternalFS;
use crate::fs::tag::TagFS;
use crate::model::TagObject;
use cli::Args;

mod file;
mod model;
mod fs;

mod cli;
const APP_ID: &str = "me.melijn.TagFS";
fn main() -> std::io::Result<()> {
    setup_logger();

    let args = Args::parse();

    let source_path = args.source_path.as_str();

    let mut fs = match TagFS::new_from_save(ExternalFS::new(source_path)) {
        Ok(fs) => fs,
        Err(e) => {
            error!("Couldn't recover FS from savefile: {e}, creating empty FS");
            TagFS::new(ExternalFS::new(source_path))
        }
    };

    let files = std::fs::read_dir(source_path)?
        .filter_map(|e| {
            e.ok()
                .filter(|e| e.file_type().map(|ft| ft.is_file()).unwrap_or(false))
        })
        .map(|e| e.file_name());

    fs.repopulate(files);


    // fuser::mount2(
    //     fs,
    //     args.mount_path,
    //     &[MountOption::AutoUnmount, MountOption::AllowRoot],
    // )?;

    // Create a new application
    let app = Application::builder().application_id(APP_ID).build();
    // Connect to "activate" signal of `app`
    app.connect_activate(build_ui);
    // Run the application without it malding about our cool arguments
    app.run_with_args::<String>(&[]);

    Ok(())
}

fn setup_logger() {
    // Create a new `env_logger::Builder`
    let mut builder = Builder::new();

    // Set the minimum log level to `Debug`
    builder.filter_level(LevelFilter::Debug);

    // Configure the log format
    builder.format_timestamp_secs();

    // Initialize the logger
    builder.init();
}

fn stateless(name: &str) -> gio::SimpleAction {
    gio::SimpleAction::new(name, None)
}

pub fn add_actions(win: &ApplicationWindow, search_bar: &SearchEntry) {
    // app.add_action(&{
    //     let a = stateless("open-actuals");
    //     a.connect_activate(glib::clone!(@weak win => move |_, _| on_open_actuals(win)));
    //     a
    // });

    win.add_action(&{
        let a = stateless("search");
        a.connect_activate(move |_, _| {
            println!("Searching");

            // let sender = crate::SENDER.get().unwrap();
            // sender.send(AppMsg::ActivateSearch).unwrap();
        });
        a
    });
    // win.add_action(&gio::PropertyAction::new("win.search", search_bar, "visible"));
    // win.add_action(&gio::PropertyAction::new("show-manage", &manage_box, "visible"));

    let shortcut = gtk::ShortcutController::new();
    shortcut.set_scope(ShortcutScope::Global);
    shortcut.add_shortcut(Shortcut::new(Some(gtk::KeyvalTrigger::new(Key::F, ModifierType::CONTROL_MASK)), Some(gtk::NamedAction::new("win.search"))));
    shortcut.add_shortcut(Shortcut::new(Some(gtk::KeyvalTrigger::new(Key::D, ModifierType::CONTROL_MASK)), Some(gtk::NamedAction::new("win.qadd"))));
    win.add_controller(shortcut);
}



fn build_ui(app: &Application) {
    let layout = gtk4::ListBox::builder()
        .build();
    let scroll = ScrolledWindow::builder()
        .child(&layout)
        .build();

    // Create a window and set the title
    let window = ApplicationWindow::builder()
        .application(app)
        .title("My GTK App")
        .content(&scroll)
        .build();

    add_video(&layout);

    let search_entry = SearchEntry::builder()
        .placeholder_text("search")
        .build();

    layout.append(&search_entry);

    add_actions(&window, &search_entry);

    // let tags = gtk4::Box::builder().build();
    // Create a `Vec<IntegerObject>` with numbers from 0 to 100_000
    let vector: Vec<TagObject> = (0..=1000).map(TagObject::new).collect();

    // Create new model
    let model = gio::ListStore::new::<TagObject>();

    // Add the vector to the model
    model.extend_from_slice(&vector);
    let factory = SignalListItemFactory::new();
    factory.connect_setup(move |_, list_item| {
        let label = Button::new();
        list_item
            .downcast_ref::<ListItem>()
            .expect("Needs to be ListItem")
            .set_child(Some(&label));
    });

    factory.connect_bind(move |_, list_item| {
        // Get `IntegerObject` from `ListItem`
        let tag_object = list_item
            .downcast_ref::<ListItem>()
            .expect("Needs to be ListItem")
            .item()
            .and_downcast::<TagObject>()
            .expect("The item has to be an `TagObject`.");

        // Get `Label` from `ListItem`
        let label = list_item
            .downcast_ref::<ListItem>()
            .expect("Needs to be ListItem")
            .child()
            .and_downcast::<Button>()
            .expect("The child has to be a `Button`.");

        // Set "label" to "number"
        label.set_label(&tag_object.tag().to_string());
    });

    let selection_model = SingleSelection::new(Some(model));
    let grid_view = ListView::new(Some(selection_model), Some(factory));

    layout.append(&grid_view);

    // Present window
    window.present();
}

fn add_search(layout: &ListBox) {

}

fn add_video(layout: &ListBox) {
    let video = Video::builder()
        .file(&gio::File::for_path("/drive2/Seafile/private/meems/raZ8yb7L_tnzmWHm.mp4"))
        .height_request(200)
        .build();

    layout.append(&video);
}
