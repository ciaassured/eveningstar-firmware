//! Descriptions of entry types, and the check that a new description keeps
//! every released one readable.
//!
//! A crate that defines entry types lists them as [`Type`]s, and commits the
//! text [`render`] writes for the last release next to them; a test runs
//! [`check`] of the committed text against the current list. Compatible
//! means:
//!
//! - every released type is still there, with the same name and minimum
//!   length;
//! - every released field is still there, at the same offset and length;
//! - a new field starts at or after the end of every released field of its
//!   type and at or after its minimum length, so a value an older firmware
//!   wrote ends before it;
//! - new types may be added.
//!
//! [`validate`] checks a list on its own: types unique by number and name,
//! fields unique by name, in order and not overlapping, none across the
//! minimum length, and names that are one word.

use core::fmt;

/// A field of an entry type: its name, offset and length in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    /// The field's name.
    pub name: &'static str,
    /// Its offset in the value.
    pub offset: u16,
    /// Its length.
    pub len: u16,
}

impl Field {
    /// The offset just past the field.
    #[must_use]
    pub const fn end(&self) -> u32 {
        self.offset as u32 + self.len as u32
    }
}

/// An entry type: its number, name, minimum length and fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Type {
    /// The type's number.
    pub kind: u16,
    /// The type's name.
    pub name: &'static str,
    /// The shortest value a reader accepts: the end of the fields the type
    /// was first released with.
    pub min_len: u16,
    /// The fields, in order of offset.
    pub fields: &'static [Field],
}

/// Why a list of types isn't valid, or isn't compatible with a released one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Incompatibility {
    /// A line of the released text doesn't read.
    Malformed {
        /// The line's number, from 1.
        line: usize,
    },
    /// Two types share a number or a name.
    DuplicateType {
        /// One of the types' number.
        kind: u16,
    },
    /// Two fields of a type overlap, or are out of order.
    Overlap {
        /// The type's number.
        kind: u16,
        /// The later field.
        field: &'static str,
    },
    /// Two fields of a type share a name.
    DuplicateField {
        /// The type's number.
        kind: u16,
        /// The field's name.
        field: &'static str,
    },
    /// A field starts before the type's minimum length and ends after it.
    AcrossMinLen {
        /// The type's number.
        kind: u16,
        /// The field.
        field: &'static str,
    },
    /// A type's or field's name is empty or holds whitespace.
    Name {
        /// The type's number.
        kind: u16,
    },
    /// A released type is gone.
    Removed {
        /// The type's number.
        kind: u16,
    },
    /// A released type has another name.
    Renamed {
        /// The type's number.
        kind: u16,
    },
    /// A released type's minimum length changed.
    MinLen {
        /// The type's number.
        kind: u16,
    },
    /// A released field is gone, renamed, moved or resized.
    FieldChanged {
        /// The type's number.
        kind: u16,
        /// The released field's line.
        line: usize,
    },
    /// A new field starts before the end of a released field or before the
    /// type's minimum length.
    NotAppended {
        /// The type's number.
        kind: u16,
        /// The new field.
        field: &'static str,
    },
}

impl fmt::Display for Incompatibility {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed { line } => write!(f, "line {line} of the released list doesn't read"),
            Self::DuplicateType { kind } => write!(f, "type {kind} is listed twice"),
            Self::Overlap { kind, field } => {
                write!(f, "type {kind}: field {field} overlaps or is out of order")
            }
            Self::DuplicateField { kind, field } => {
                write!(f, "type {kind}: field {field} is listed twice")
            }
            Self::AcrossMinLen { kind, field } => {
                write!(f, "type {kind}: field {field} crosses the minimum length")
            }
            Self::Name { kind } => write!(f, "type {kind}: a name isn't one word"),
            Self::Removed { kind } => write!(f, "released type {kind} is gone"),
            Self::Renamed { kind } => write!(f, "released type {kind} was renamed"),
            Self::MinLen { kind } => write!(f, "released type {kind} changed its minimum length"),
            Self::FieldChanged { kind, line } => {
                write!(f, "type {kind}: the released field of line {line} changed")
            }
            Self::NotAppended { kind, field } => {
                write!(
                    f,
                    "type {kind}: new field {field} isn't after the released ones and the minimum length"
                )
            }
        }
    }
}

/// Checks `types` on its own: no number or name twice; each type's fields
/// unique by name, in order of offset, without overlap, and none starting
/// before the minimum length and ending after it; every name one word.
///
/// # Errors
///
/// The first [`Incompatibility`] found.
pub fn validate(types: &[Type]) -> Result<(), Incompatibility> {
    for (i, ty) in types.iter().enumerate() {
        let twice = types
            .iter()
            .skip(i + 1)
            .any(|other| other.kind == ty.kind || other.name == ty.name);
        if twice {
            return Err(Incompatibility::DuplicateType { kind: ty.kind });
        }
        validate_fields(ty)?;
    }
    Ok(())
}

/// Checks the fields and the names of `ty`, as [`validate`] describes.
fn validate_fields(ty: &Type) -> Result<(), Incompatibility> {
    let kind = ty.kind;
    if !is_word(ty.name) {
        return Err(Incompatibility::Name { kind });
    }
    let min = u32::from(ty.min_len);
    let mut end = 0;
    for (i, field) in ty.fields.iter().enumerate() {
        let name = field.name;
        if !is_word(name) {
            return Err(Incompatibility::Name { kind });
        }
        if ty.fields.iter().skip(i + 1).any(|other| other.name == name) {
            return Err(Incompatibility::DuplicateField { kind, field: name });
        }
        if u32::from(field.offset) < end {
            return Err(Incompatibility::Overlap { kind, field: name });
        }
        if u32::from(field.offset) < min && field.end() > min {
            return Err(Incompatibility::AcrossMinLen { kind, field: name });
        }
        end = field.end();
    }
    Ok(())
}

/// Whether `name` is one word: not empty, and without whitespace.
fn is_word(name: &str) -> bool {
    !name.is_empty() && !name.contains(char::is_whitespace)
}

/// Writes `types` in the text form [`check`] reads:
///
/// ```text
/// type <kind> <name> min <min_len>
///   field <name> <offset> <len>
/// ```
///
/// # Errors
///
/// When `out` fails.
pub fn render(types: &[Type], out: &mut impl fmt::Write) -> fmt::Result {
    for ty in types {
        writeln!(out, "type {} {} min {}", ty.kind, ty.name, ty.min_len)?;
        for field in ty.fields {
            writeln!(out, "  field {} {} {}", field.name, field.offset, field.len)?;
        }
    }
    Ok(())
}

/// Checks `current` with [`validate`], then that it keeps every type and
/// field of `released`, the text [`render`] wrote for the last release, as
/// the module describes.
///
/// # Errors
///
/// The first [`Incompatibility`] found.
pub fn check(released: &str, current: &[Type]) -> Result<(), Incompatibility> {
    validate(current)?;
    // The type being read, with the index of its line.
    let mut open: Option<(&Type, usize)> = None;
    for (index, line) in released.lines().enumerate() {
        let next = read_line(line, index, open, current)?;
        if let (Some((previous, at)), Some((_, now))) = (open, next)
            && now != at
        {
            appended(released, at, previous)?;
        }
        open = next;
    }
    if let Some((last, at)) = open {
        appended(released, at, last)?;
    }
    Ok(())
}

/// Checks the line at index `index` of the released text against `current`,
/// with `open` the type its `field` lines belong to, and returns the type
/// open after it. Never inlined, so the line's iterators stay out of
/// [`check`]'s frame.
#[inline(never)]
fn read_line<'t>(
    line: &str,
    index: usize,
    open: Option<(&'t Type, usize)>,
    current: &'t [Type],
) -> Result<Option<(&'t Type, usize)>, Incompatibility> {
    let number = index + 1;
    let mut words = line.split_whitespace();
    match words.next() {
        Some("type") => {
            let (kind, name, min) =
                type_line(&mut words).ok_or(Incompatibility::Malformed { line: number })?;
            let found = current
                .iter()
                .find(|candidate| candidate.kind == kind)
                .ok_or(Incompatibility::Removed { kind })?;
            if found.name != name {
                return Err(Incompatibility::Renamed { kind });
            }
            if found.min_len != min {
                return Err(Incompatibility::MinLen { kind });
            }
            Ok(Some((found, index)))
        }
        Some("field") => {
            let (Some((ty, _)), Some((name, offset, len))) = (open, field_line(&mut words)) else {
                return Err(Incompatibility::Malformed { line: number });
            };
            let kept = ty
                .fields
                .iter()
                .any(|field| field.name == name && field.offset == offset && field.len == len);
            if !kept {
                return Err(Incompatibility::FieldChanged {
                    kind: ty.kind,
                    line: number,
                });
            }
            Ok(open)
        }
        // `render` writes no blank lines, and a blank line would end the
        // released fields `appended` reads.
        None | Some(_) => Err(Incompatibility::Malformed { line: number }),
    }
}

/// Checks that every field of `ty` that isn't among its released fields, the
/// `field` lines after line index `at` of `released`, starts at or after the
/// end of all of them and at or after the type's minimum length. Never inlined, so its iterators stay out of
/// [`check`]'s frame.
#[inline(never)]
fn appended(released: &str, at: usize, ty: &Type) -> Result<(), Incompatibility> {
    let fields = || {
        released
            .lines()
            .skip(at + 1)
            .map_while(|line| {
                let mut words = line.split_whitespace();
                match words.next() {
                    Some("field") => Some(field_line(&mut words)),
                    _ => None,
                }
            })
            .flatten()
    };
    let end = fields()
        .map(|(_, offset, len)| u32::from(offset) + u32::from(len))
        .max()
        .unwrap_or(0)
        .max(u32::from(ty.min_len));
    for field in ty.fields {
        let released_field = fields().any(|(name, offset, len)| {
            name == field.name && offset == field.offset && len == field.len
        });
        if !released_field && u32::from(field.offset) < end {
            return Err(Incompatibility::NotAppended {
                kind: ty.kind,
                field: field.name,
            });
        }
    }
    Ok(())
}

/// The kind, name and minimum length of a `type` line's remaining words.
fn type_line<'a>(words: &mut impl Iterator<Item = &'a str>) -> Option<(u16, &'a str, u16)> {
    let kind = words.next()?.parse().ok()?;
    let name = words.next()?;
    if words.next()? != "min" {
        return None;
    }
    let min = words.next()?.parse().ok()?;
    words.next().is_none().then_some((kind, name, min))
}

/// The name, offset and length of a `field` line's remaining words.
fn field_line<'a>(words: &mut impl Iterator<Item = &'a str>) -> Option<(&'a str, u16, u16)> {
    let name = words.next()?;
    let offset = words.next()?.parse().ok()?;
    let len = words.next()?.parse().ok()?;
    words.next().is_none().then_some((name, offset, len))
}

#[cfg(test)]
mod tests;
