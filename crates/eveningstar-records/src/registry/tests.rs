use super::{Field, Incompatibility, Type, check, render, validate};

const A: Field = Field {
    name: "a",
    offset: 0,
    len: 4,
};
const B: Field = Field {
    name: "b",
    offset: 4,
    len: 4,
};
const C: Field = Field {
    name: "c",
    offset: 8,
    len: 2,
};

const RELEASED: &[Type] = &[Type {
    kind: 1,
    name: "board",
    min_len: 8,
    fields: &[A, B],
}];

fn released_text() -> String {
    let mut text = String::new();
    render(RELEASED, &mut text).unwrap();
    text
}

#[test]
fn render_writes_the_text_check_reads() {
    assert_eq!(
        released_text(),
        "type 1 board min 8\n  field a 0 4\n  field b 4 4\n"
    );
    assert_eq!(check(&released_text(), RELEASED), Ok(()));
}

#[test]
fn appending_fields_and_types_is_compatible() {
    let current = [
        Type {
            kind: 1,
            name: "board",
            min_len: 8,
            fields: &[A, B, C],
        },
        Type {
            kind: 2,
            name: "trust",
            min_len: 4,
            fields: &[A],
        },
    ];
    assert_eq!(check(&released_text(), &current), Ok(()));
}

#[test]
fn removing_renaming_or_resizing_is_refused() {
    let text = released_text();
    assert_eq!(check(&text, &[]), Err(Incompatibility::Removed { kind: 1 }));
    let renamed = [Type {
        name: "boards",
        ..RELEASED[0]
    }];
    assert_eq!(
        check(&text, &renamed),
        Err(Incompatibility::Renamed { kind: 1 })
    );
    let raised = [Type {
        min_len: 12,
        ..RELEASED[0]
    }];
    assert_eq!(
        check(&text, &raised),
        Err(Incompatibility::MinLen { kind: 1 })
    );
    let resized = [Type {
        fields: &[A, Field { len: 2, ..B }],
        ..RELEASED[0]
    }];
    assert_eq!(
        check(&text, &resized),
        Err(Incompatibility::FieldChanged { kind: 1, line: 3 })
    );
    let dropped = [Type {
        fields: &[A],
        ..RELEASED[0]
    }];
    assert_eq!(
        check(&text, &dropped),
        Err(Incompatibility::FieldChanged { kind: 1, line: 3 })
    );
}

#[test]
fn a_field_inserted_among_released_ones_is_refused() {
    let text = "type 1 board min 8\n  field a 0 2\n  field b 4 4\n";
    let inserted = [Type {
        kind: 1,
        name: "board",
        min_len: 8,
        fields: &[
            Field {
                name: "a",
                offset: 0,
                len: 2,
            },
            Field {
                name: "x",
                offset: 2,
                len: 2,
            },
            B,
        ],
    }];
    assert_eq!(
        check(text, &inserted),
        Err(Incompatibility::NotAppended {
            kind: 1,
            field: "x"
        })
    );
}

#[test]
fn a_list_with_duplicates_or_overlaps_is_invalid() {
    let twice = [RELEASED[0], RELEASED[0]];
    assert_eq!(
        validate(&twice),
        Err(Incompatibility::DuplicateType { kind: 1 })
    );
    let overlapping = [Type {
        fields: &[A, Field { offset: 2, ..B }],
        ..RELEASED[0]
    }];
    assert_eq!(
        validate(&overlapping),
        Err(Incompatibility::Overlap {
            kind: 1,
            field: "b"
        })
    );
}

#[test]
fn malformed_lines_are_refused() {
    for (text, line) in [
        ("type x board min 8\n", 1),
        ("type 1 board 8\n", 1),
        ("  field a 0 4\n", 1),
        ("type 1 board min 8\n  field a 0\n", 2),
        ("type 1 board min 8\nsomething\n", 2),
        ("type 1 board min 8\n\n  field a 0 4\n", 2),
    ] {
        assert_eq!(
            check(text, RELEASED),
            Err(Incompatibility::Malformed { line }),
            "{text:?}"
        );
    }
}

#[test]
fn a_field_inserted_into_an_earlier_type_is_refused() {
    let text =
        "type 1 board min 4\n  field a 0 4\n  field c 8 2\ntype 2 trust min 4\n  field a 0 4\n";
    let current = [
        Type {
            kind: 1,
            name: "board",
            min_len: 4,
            fields: &[A, B, C],
        },
        Type {
            kind: 2,
            name: "trust",
            min_len: 4,
            fields: &[A],
        },
    ];
    assert_eq!(
        check(text, &current),
        Err(Incompatibility::NotAppended {
            kind: 1,
            field: "b"
        })
    );
}

#[test]
fn a_new_field_within_the_minimum_length_is_refused() {
    let text = "type 1 board min 8\n  field a 0 4\n";
    let current = [Type {
        kind: 1,
        name: "board",
        min_len: 8,
        fields: &[A, B],
    }];
    assert_eq!(
        check(text, &current),
        Err(Incompatibility::NotAppended {
            kind: 1,
            field: "b"
        })
    );
}

#[test]
fn duplicate_fields_bad_names_and_fields_across_the_minimum_are_invalid() {
    let twice = [Type {
        fields: &[A, Field { offset: 8, ..A }],
        ..RELEASED[0]
    }];
    assert_eq!(
        validate(&twice),
        Err(Incompatibility::DuplicateField {
            kind: 1,
            field: "a"
        })
    );
    let across = [Type {
        min_len: 6,
        ..RELEASED[0]
    }];
    assert_eq!(
        validate(&across),
        Err(Incompatibility::AcrossMinLen {
            kind: 1,
            field: "b"
        })
    );
    for (name, field) in [
        ("two words", "a"),
        ("", "a"),
        ("board", "a b"),
        ("board", ""),
    ] {
        let named = [Type {
            name,
            fields: Box::leak(Box::new([Field { name: field, ..A }])),
            ..RELEASED[0]
        }];
        assert_eq!(validate(&named), Err(Incompatibility::Name { kind: 1 }));
    }
}
