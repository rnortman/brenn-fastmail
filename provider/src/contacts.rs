//! The names the contacts tools give a card's lists, shared by the read
//! answer's members and the update's `list` argument, so the name a read
//! shows is the name an update takes.

use fastmail_jmap::EntryList;

/// `list`'s name in the contacts tools' answers and arguments.
pub fn list_name(list: EntryList) -> &'static str {
    match list {
        EntryList::Emails => "emails",
        EntryList::Phones => "phones",
        EntryList::OnlineServices => "online_services",
        EntryList::Addresses => "addresses",
        EntryList::Organizations => "organizations",
        EntryList::Titles => "titles",
        EntryList::Nicknames => "nicknames",
        EntryList::Anniversaries => "anniversaries",
        EntryList::Notes => "notes",
    }
}
