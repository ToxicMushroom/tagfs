use crate::file::TagNumber;
use std::cell::Cell;

use glib::Properties;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;


// Object holding the state
#[derive(Properties, Default)]
#[properties(wrapper_type = super::TagObject)]
pub struct TagObject {
    #[property(get, set)]
    tag: Cell<TagNumber>,
}

// The central trait for subclassing a GObject
#[glib::object_subclass]
impl ObjectSubclass for TagObject {
    const NAME: &'static str = "MyGtkAppTagObject";
    type Type = super::TagObject;
}

// Trait shared by all GObjects
#[glib::derived_properties]
impl ObjectImpl for TagObject {}