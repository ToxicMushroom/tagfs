mod imp;

use glib::Object;
use gtk4::glib;

use crate::file::TagNumber;


glib::wrapper! {
    pub struct TagObject(ObjectSubclass<imp::TagObject>);
}

impl TagObject {
    pub fn new(tag: TagNumber) -> Self {
        Object::builder().property("tag", tag).build()
    }
}