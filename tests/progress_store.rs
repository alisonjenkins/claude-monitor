#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Store API behind plan bindings: one binding per owner, steps that are
//! replaced wholesale and idempotently (data-model: Step, ProgressBinding).

use claude_monitor::model::{OwnerKind, StepStatus};
use claude_monitor::progress::{Item, Source, SourceKind};
use claude_monitor::store::progress::ChosenBy;
use claude_monitor::store::{Store, StoreOptions};
use tempfile::TempDir;

const T0: &str = "2026-10-12T10:00:00Z";
const T1: &str = "2026-10-12T11:00:00Z";

fn store() -> (TempDir, Store) {
    let dir = TempDir::new().unwrap();
    let store = Store::open_in_dir(dir.path(), &StoreOptions::default()).unwrap();
    (dir, store)
}

fn speckit(path: &str) -> Source {
    Source {
        kind: SourceKind::SpeckitTasks,
        path: Some(path.to_owned()),
    }
}

fn item(ordinal: u32, text: &str, status: StepStatus) -> Item {
    Item {
        ordinal,
        text: text.to_owned(),
        status,
        group: Some("Phase 1".to_owned()),
        external_id: Some(format!("T{ordinal:03}")),
    }
}

#[test]
#[ignore = "implemented by T088"]
fn no_binding_until_one_is_made() {
    let (_dir, store) = store();

    assert_eq!(
        store
            .get_progress_binding(OwnerKind::Session, "s1")
            .unwrap(),
        None
    );
}

#[test]
#[ignore = "implemented by T088"]
fn a_binding_round_trips() {
    let (_dir, store) = store();

    let made = store
        .bind_progress(
            OwnerKind::Session,
            "s1",
            &speckit("/r/tasks.md"),
            ChosenBy::User,
            T0,
        )
        .unwrap();
    let read = store
        .get_progress_binding(OwnerKind::Session, "s1")
        .unwrap()
        .unwrap();

    assert_eq!(read, made);
    assert!(!read.id.is_empty());
    assert_eq!(read.source, speckit("/r/tasks.md"));
    assert_eq!(read.chosen_by, ChosenBy::User);
    assert_eq!(read.read_at, T0);
}

#[test]
#[ignore = "implemented by T088"]
fn rebinding_replaces_the_one_binding_and_drops_old_steps() {
    let (_dir, store) = store();
    let first = store
        .bind_progress(
            OwnerKind::Session,
            "s1",
            &speckit("/r/a.md"),
            ChosenBy::Auto,
            T0,
        )
        .unwrap();
    store
        .replace_steps(&first.id, &[item(0, "one", StepStatus::Todo)])
        .unwrap();

    let second = store
        .bind_progress(
            OwnerKind::Session,
            "s1",
            &speckit("/r/b.md"),
            ChosenBy::User,
            T1,
        )
        .unwrap();

    assert_eq!(second.id, first.id);
    assert_eq!(second.source, speckit("/r/b.md"));
    assert_eq!(second.chosen_by, ChosenBy::User);
    assert!(store.list_steps(&second.id).unwrap().is_empty());
}

#[test]
#[ignore = "implemented by T088"]
fn rebinding_the_same_source_keeps_its_steps() {
    let (_dir, store) = store();
    let first = store
        .bind_progress(
            OwnerKind::Session,
            "s1",
            &speckit("/r/a.md"),
            ChosenBy::Auto,
            T0,
        )
        .unwrap();
    store
        .replace_steps(&first.id, &[item(0, "one", StepStatus::Todo)])
        .unwrap();

    let again = store
        .bind_progress(
            OwnerKind::Session,
            "s1",
            &speckit("/r/a.md"),
            ChosenBy::User,
            T1,
        )
        .unwrap();

    assert_eq!(again.chosen_by, ChosenBy::User);
    assert_eq!(store.list_steps(&again.id).unwrap().len(), 1);
}

#[test]
#[ignore = "implemented by T088"]
fn bindings_are_per_owner() {
    let (_dir, store) = store();
    store
        .bind_progress(
            OwnerKind::Session,
            "s1",
            &speckit("/r/a.md"),
            ChosenBy::Auto,
            T0,
        )
        .unwrap();

    assert!(store
        .get_progress_binding(OwnerKind::Session, "s2")
        .unwrap()
        .is_none());
    assert!(store
        .get_progress_binding(OwnerKind::Project, "s1")
        .unwrap()
        .is_none());
}

#[test]
#[ignore = "implemented by T088"]
fn unbinding_removes_the_binding_and_its_steps() {
    let (_dir, store) = store();
    let binding = store
        .bind_progress(
            OwnerKind::Session,
            "s1",
            &speckit("/r/a.md"),
            ChosenBy::Auto,
            T0,
        )
        .unwrap();
    store
        .replace_steps(&binding.id, &[item(0, "one", StepStatus::Done)])
        .unwrap();

    assert!(store.unbind_progress(OwnerKind::Session, "s1").unwrap());

    assert!(store
        .get_progress_binding(OwnerKind::Session, "s1")
        .unwrap()
        .is_none());
    assert!(store.list_steps(&binding.id).unwrap().is_empty());
    assert!(!store.unbind_progress(OwnerKind::Session, "s1").unwrap());
}

#[test]
#[ignore = "implemented by T088"]
fn steps_round_trip_in_order() {
    let (_dir, store) = store();
    let binding = store
        .bind_progress(
            OwnerKind::Session,
            "s1",
            &speckit("/r/a.md"),
            ChosenBy::Auto,
            T0,
        )
        .unwrap();
    let items = vec![
        item(0, "one", StepStatus::Done),
        item(1, "two", StepStatus::Todo),
        Item {
            group: None,
            external_id: None,
            ..item(2, "three", StepStatus::Doing)
        },
    ];

    assert!(store.replace_steps(&binding.id, &items).unwrap());

    assert_eq!(store.list_steps(&binding.id).unwrap(), items);
}

#[test]
#[ignore = "implemented by T088"]
fn replacing_steps_is_idempotent() {
    let (_dir, store) = store();
    let binding = store
        .bind_progress(
            OwnerKind::Session,
            "s1",
            &speckit("/r/a.md"),
            ChosenBy::Auto,
            T0,
        )
        .unwrap();
    let items = vec![item(0, "one", StepStatus::Todo)];
    assert!(store.replace_steps(&binding.id, &items).unwrap());

    assert!(!store.replace_steps(&binding.id, &items).unwrap());

    let changed = vec![item(0, "one", StepStatus::Done)];
    assert!(store.replace_steps(&binding.id, &changed).unwrap());
    assert_eq!(store.list_steps(&binding.id).unwrap(), changed);
}

#[test]
#[ignore = "implemented by T088"]
fn shrinking_a_plan_removes_the_extra_steps() {
    let (_dir, store) = store();
    let binding = store
        .bind_progress(
            OwnerKind::Session,
            "s1",
            &speckit("/r/a.md"),
            ChosenBy::Auto,
            T0,
        )
        .unwrap();
    store
        .replace_steps(
            &binding.id,
            &[
                item(0, "one", StepStatus::Todo),
                item(1, "two", StepStatus::Todo),
            ],
        )
        .unwrap();

    store
        .replace_steps(&binding.id, &[item(0, "one", StepStatus::Todo)])
        .unwrap();

    assert_eq!(store.list_steps(&binding.id).unwrap().len(), 1);
}

#[test]
#[ignore = "implemented by T088"]
fn touching_a_binding_moves_its_read_time() {
    let (_dir, store) = store();
    let binding = store
        .bind_progress(
            OwnerKind::Session,
            "s1",
            &speckit("/r/a.md"),
            ChosenBy::Auto,
            T0,
        )
        .unwrap();

    store.touch_progress_binding(&binding.id, T1).unwrap();

    let read = store
        .get_progress_binding(OwnerKind::Session, "s1")
        .unwrap()
        .unwrap();
    assert_eq!(read.read_at, T1);
}

#[test]
#[ignore = "implemented by T088"]
fn a_binding_survives_reopening_the_store() {
    let dir = TempDir::new().unwrap();
    {
        let store = Store::open_in_dir(dir.path(), &StoreOptions::default()).unwrap();
        let binding = store
            .bind_progress(
                OwnerKind::Session,
                "s1",
                &speckit("/r/a.md"),
                ChosenBy::User,
                T0,
            )
            .unwrap();
        store
            .replace_steps(&binding.id, &[item(0, "one", StepStatus::Todo)])
            .unwrap();
    }

    let store = Store::open_in_dir(dir.path(), &StoreOptions::default()).unwrap();
    let binding = store
        .get_progress_binding(OwnerKind::Session, "s1")
        .unwrap()
        .unwrap();

    assert_eq!(binding.chosen_by, ChosenBy::User);
    assert_eq!(store.list_steps(&binding.id).unwrap().len(), 1);
}
