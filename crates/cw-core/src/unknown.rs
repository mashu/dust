//! Keys a later build adds inside a value this build reads whole, kept as
//! they were read and written back beside it: [`KeepingUnknown`] reads the
//! value and them, [`WithKeys`] writes them both.

use std::fmt;
use std::marker::PhantomData;

use serde::de::{
    self, DeserializeOwned, DeserializeSeed, Deserializer, IgnoredAny, MapAccess, SeqAccess,
    Visitor,
};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Map, Value};

/// A `T` as it is read, and the keys beside its own that it does not read,
/// as they were read.
///
/// `T`'s own reader reads every key it names just as it would read them on
/// their own: a key given twice, a key missing, a value of the wrong type,
/// a value left out for its default. So this takes what `T` takes, as `T`,
/// and refuses what `T` refuses; and beside, a value JSON's numbers or text
/// cannot hold under a key `T` does not name, a number past `f64`'s range
/// or text of half a surrogate pair: `T` passes over it unread, where it is
/// refused as it is kept, as serde's own buffer refuses it for `T` read
/// through an untagged enum or a flattened field. Only the keys `T` does not
/// name, which it would pass over, are taken out on the way and kept. It is
/// read in one pass, and nothing is held on the way but what is kept.
///
/// `T` is a struct that passes over the keys it does not name, rather than
/// refuse them, and reads none flattened.
pub(crate) struct KeepingUnknown<T> {
    pub(crate) known: T,
    pub(crate) unknown: Map<String, Value>,
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for KeepingUnknown<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(Outer(PhantomData))
    }
}

/// What reads a [`KeepingUnknown`]: an object for `T`'s reader, the keys
/// it does not name taken out on the way, or a list as `T` reads one, with
/// no keys to keep. Anything else is no `T`.
struct Outer<T>(PhantomData<T>);

impl<'de, T: Deserialize<'de>> Visitor<'de> for Outer<T> {
    type Value = KeepingUnknown<T>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a struct")
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
        let mut unknown = Map::new();
        let known = T::deserialize(Sifted {
            map,
            unknown: &mut unknown,
        })?;
        Ok(KeepingUnknown { known, unknown })
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
        let known = T::deserialize(de::value::SeqAccessDeserializer::new(seq))?;
        Ok(KeepingUnknown {
            known,
            unknown: Map::new(),
        })
    }
}

/// An object read for `T`'s reader: read as a struct, it gives that reader
/// only the keys it names, and keeps the others in `unknown`. Read as
/// anything else, every key, as the object would.
struct Sifted<'u, A> {
    map: A,
    unknown: &'u mut Map<String, Value>,
}

impl<'de, A: MapAccess<'de>> Deserializer<'de> for Sifted<'_, A> {
    type Error = A::Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, A::Error> {
        visitor.visit_map(self.map)
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, A::Error> {
        visitor.visit_map(SiftedMap {
            map: self.map,
            fields,
            unknown: self.unknown,
        })
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple
        tuple_struct map enum identifier ignored_any
    }
}

/// The entries of an object under the keys `fields` names, in their order;
/// the rest taken out into `unknown` as they come.
struct SiftedMap<'u, A> {
    map: A,
    fields: &'static [&'static str],
    unknown: &'u mut Map<String, Value>,
}

impl<'de, A: MapAccess<'de>> MapAccess<'de> for SiftedMap<'_, A> {
    type Error = A::Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, A::Error> {
        loop {
            match self.map.next_key_seed(KeyOf(self.fields))? {
                None => return Ok(None),
                Some(Key::Named(name)) => {
                    let name = de::value::BorrowedStrDeserializer::new(name);
                    return seed.deserialize(name).map(Some);
                }
                Some(Key::Other(key)) => {
                    // A key given twice is kept as given last, as an
                    // object read as a value keeps it.
                    let value = self.map.next_value()?;
                    self.unknown.insert(key, value);
                }
                Some(Key::NoText) => {
                    self.map.next_value::<IgnoredAny>()?;
                }
            }
        }
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, A::Error> {
        self.map.next_value_seed(seed)
    }
}

/// A key as [`SiftedMap`] reads it, against the keys a struct names.
struct KeyOf(&'static [&'static str]);

/// What a key read by [`KeyOf`] is.
enum Key {
    /// One the struct names, as it names it.
    Named(&'static str),
    /// One it does not.
    Other(String),
    /// Bytes that are no text and no key the struct names: passed over, as
    /// the struct's own reader passes them over. JSON has none.
    NoText,
}

impl<'de> DeserializeSeed<'de> for KeyOf {
    type Value = Key;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Key, D::Error> {
        deserializer.deserialize_identifier(self)
    }
}

impl KeyOf {
    fn named(&self, key: &[u8]) -> Option<&'static str> {
        self.0.iter().copied().find(|name| name.as_bytes() == key)
    }
}

impl Visitor<'_> for KeyOf {
    type Value = Key;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a key")
    }

    fn visit_str<E: de::Error>(self, key: &str) -> Result<Key, E> {
        Ok(self
            .named(key.as_bytes())
            .map_or_else(|| Key::Other(key.to_owned()), Key::Named))
    }

    fn visit_string<E: de::Error>(self, key: String) -> Result<Key, E> {
        Ok(self
            .named(key.as_bytes())
            .map_or(Key::Other(key), Key::Named))
    }

    fn visit_bytes<E: de::Error>(self, key: &[u8]) -> Result<Key, E> {
        Ok(match self.named(key) {
            Some(name) => Key::Named(name),
            None => std::str::from_utf8(key).map_or(Key::NoText, |key| Key::Other(key.to_owned())),
        })
    }
}

/// `known` written with the keys beside its own that it did not read, after
/// its own ([`KeepingUnknown`]): but for any key `T` reads, which would be
/// read back as `T`'s own, or be written twice and make `T` unreadable.
pub(crate) struct WithKeys<'a, T> {
    pub(crate) known: &'a T,
    pub(crate) unknown: &'a Map<String, Value>,
}

impl<T: Serialize + DeserializeOwned> Serialize for WithKeys<'_, T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.unknown.is_empty() {
            return self.known.serialize(serializer);
        }
        Both {
            known: self.known,
            unknown: NotRead {
                keys: self.unknown,
                read: keys_read::<T>(),
            },
        }
        .serialize(serializer)
    }
}

/// A value's own keys, and after them those it does not read.
#[derive(Serialize)]
#[serde(bound = "T: Serialize")]
struct Both<'a, T> {
    #[serde(flatten)]
    known: &'a T,
    #[serde(flatten)]
    unknown: NotRead<'a>,
}

/// `keys` as they are written beside a value that reads `read`: all but
/// those.
struct NotRead<'a> {
    keys: &'a Map<String, Value>,
    read: &'static [&'static str],
}

impl Serialize for NotRead<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(
            self.keys
                .iter()
                .filter(|(key, _)| !self.read.contains(&key.as_str())),
        )
    }
}

/// The keys `T`, a struct, reads, as its reader names them, its aliases
/// among them: none where it names none, as a struct read flattened.
pub(crate) fn keys_read<T: DeserializeOwned>() -> &'static [&'static str] {
    T::deserialize(Keys).err().map_or(&[], |asked| asked.0)
}

/// Asks a reader for the keys it reads, and gives none of them
/// ([`keys_read`]).
struct Keys;

/// What [`Keys`] was asked to read.
#[derive(Debug)]
struct Asked(&'static [&'static str]);

impl fmt::Display for Asked {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(formatter, "asked for {:?}", self.0)
    }
}

impl std::error::Error for Asked {}

impl de::Error for Asked {
    fn custom<M: fmt::Display>(_: M) -> Self {
        Self(&[])
    }
}

impl<'de> Deserializer<'de> for Keys {
    type Error = Asked;

    fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Asked> {
        Err(Asked(&[]))
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        keys: &'static [&'static str],
        _: V,
    ) -> Result<V::Value, Asked> {
        Err(Asked(keys))
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple
        tuple_struct map enum identifier ignored_any
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::alignment::LetterAccuracy;
    use crate::recognition::CharTimes;
    use crate::rng::{Rng, SplitMix64};

    /// Values JSON may write that its numbers or text cannot hold, as
    /// serde_json reads them: a number past `f64`'s range, and text of half
    /// a surrogate pair.
    const UNHOLDABLE: [&str; 2] = ["1e400", r#""\ud800""#];

    /// A JSON value of any kind, small, as a later build might write under
    /// a key: lists and objects of them only near the top. Now and then one
    /// JSON's numbers or text cannot hold ([`UNHOLDABLE`]).
    fn any_value(rng: &mut SplitMix64, depth: u32) -> String {
        let kinds = if depth < 2 { 7 } else { 4 };
        match rng.usize_in(0, kinds) {
            0 => "null".to_string(),
            1 => (rng.f64() < 0.5).to_string(),
            2 if rng.f64() < 0.03 => UNHOLDABLE[0].to_string(),
            2 => rng.usize_in(0, 70_000).to_string(),
            3 => format!("-{}.5", rng.usize_in(0, 9)),
            4 if rng.f64() < 0.03 => UNHOLDABLE[1].to_string(),
            4 => serde_json::to_string(&format!("s{}", rng.usize_in(0, 9))).unwrap(),
            5 => {
                let items: Vec<String> = (0..rng.usize_in(0, 3))
                    .map(|_| any_value(rng, depth + 1))
                    .collect();
                format!("[{}]", items.join(","))
            }
            _ => {
                let entries: Vec<String> = (0..rng.usize_in(0, 3))
                    .map(|_| format!("{}:{}", any_key(rng), any_value(rng, depth + 1)))
                    .collect();
                format!("{{{}}}", entries.join(","))
            }
        }
    }

    /// A key no struct here reads: some of them a key it reads but for
    /// its case, a space, or a letter, and one empty.
    fn any_key(rng: &mut SplitMix64) -> String {
        const KEYS: &[&str] = &[
            "later",
            "laterInside",
            "Ends",
            "ends ",
            "box_open",
            "latency",
            "correctly",
            "totals",
            "",
            "\u{e9}t\u{e9}",
        ];
        serde_json::to_string(KEYS[rng.usize_in(0, KEYS.len() - 1)]).unwrap()
    }

    /// A value read under `key`, one of [`CharTimes`]'s or
    /// [`LetterAccuracy`]'s, as its reader takes it.
    fn read_under(key: &str, rng: &mut SplitMix64) -> String {
        let numbers = |rng: &mut SplitMix64, or_none: bool| {
            let items: Vec<String> = (0..rng.usize_in(0, 4))
                .map(|_| {
                    if or_none && rng.f64() < 0.3 {
                        "null".to_string()
                    } else {
                        rng.usize_in(0, 9_000).to_string()
                    }
                })
                .collect();
            format!("[{}]", items.join(","))
        };
        match key {
            "ends" | "corrected" => numbers(rng, false),
            "keys" => numbers(rng, true),
            "latencyMs" | "correct" | "total" => rng.usize_in(0, 9_000).to_string(),
            "boxOpen" => (rng.f64() < 0.5).to_string(),
            _ => panic!("a value to read under {key}: name one here"),
        }
    }

    /// What a later build, or a damaged store, might write in place of a
    /// struct that reads `keys`: an object of keys it reads, with values
    /// it reads or not, beside keys it does not, a key given twice now and
    /// then; or, now and then, a list of what it reads, or anything else.
    pub(crate) fn written_for(keys: &[&str], rng: &mut SplitMix64) -> String {
        match rng.usize_in(0, 19) {
            0 => return any_value(rng, 0),
            1 => {
                let items: Vec<String> = keys
                    .iter()
                    .take(rng.usize_in(0, keys.len()))
                    .map(|key| read_under(key, rng))
                    .collect();
                return format!("[{}]", items.join(","));
            }
            _ => {}
        }
        let mut entries: Vec<String> = Vec::new();
        for key in keys {
            if rng.f64() < 0.9 {
                let value = if rng.f64() < 0.95 {
                    read_under(key, rng)
                } else {
                    any_value(rng, 0)
                };
                entries.push(format!("\"{key}\":{value}"));
            }
        }
        for _ in 0..rng.usize_in(0, 3) {
            entries.push(format!("{}:{}", any_key(rng), any_value(rng, 0)));
        }
        if !entries.is_empty() && rng.f64() < 0.1 {
            let twice = entries[rng.usize_in(0, entries.len() - 1)].clone();
            entries.push(twice);
        }
        for at in (1..entries.len()).rev() {
            entries.swap(at, rng.usize_in(0, at));
        }
        format!("{{{}}}", entries.join(","))
    }

    /// What `T` reads `text` as, read directly, through an untagged enum,
    /// and flattened: the three ways a session's reader reaches a value,
    /// the last two each holding it first as serde's own buffer.
    fn three_ways<T: DeserializeOwned>(text: &str) -> [serde_json::Result<T>; 3] {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Untagged<T> {
            One(T),
        }
        #[derive(Deserialize)]
        struct Flattened<T> {
            #[serde(flatten)]
            inner: Inner<T>,
        }
        #[derive(Deserialize)]
        struct Inner<T> {
            value: T,
        }
        [
            serde_json::from_str::<T>(text),
            serde_json::from_str::<Untagged<T>>(text).map(|Untagged::One(read)| read),
            serde_json::from_str::<Flattened<T>>(&format!("{{\"value\":{text}}}"))
                .map(|read| read.inner.value),
        ]
    }

    /// For 20,000 values of each kind written as [`written_for`] writes
    /// them, [`KeepingUnknown`] reads what `T` reads, and refuses what it
    /// refuses, read in each of the ways a session's reader reads them; and
    /// it keeps every key of an object that `T` does not read, as the
    /// object holds it, and none that it does. But for one thing, read
    /// directly: a value JSON's numbers or text cannot hold
    /// ([`UNHOLDABLE`]), which `T` passes over unread under a key it does
    /// not read, and it refuses. Beside it, how many read, and how many
    /// such it refused.
    fn reads_as_the_typed_reader<T>(seed: u64) -> (usize, usize)
    where
        T: DeserializeOwned + PartialEq + fmt::Debug,
    {
        let read = keys_read::<T>();
        assert!(!read.is_empty());
        let mut rng = SplitMix64::new(seed);
        let (mut took, mut refused) = (0, 0);
        for _ in 0..20_000 {
            let text = written_for(read, &mut rng);
            // Only a value it cannot hold keeps JSON from reading it.
            let held = serde_json::from_str::<Value>(&text);
            let unholdable = held.is_err();
            let beside = match held {
                Ok(Value::Object(mut beside)) => {
                    beside.retain(|key, _| !read.contains(&key.as_str()));
                    beside
                }
                _ => Map::new(),
            };
            let typed = three_ways::<T>(&text);
            let keeping = three_ways::<KeepingUnknown<T>>(&text);
            for (way, (typed, keeping)) in typed.into_iter().zip(keeping).enumerate() {
                match (typed, keeping) {
                    (Ok(typed), Ok(keeping)) => {
                        took += 1;
                        assert_eq!(keeping.known, typed, "{text}");
                        assert_eq!(keeping.unknown, beside, "{text}");
                    }
                    (Err(_), Err(_)) => {}
                    (Ok(_), Err(_)) if way == 0 && unholdable => refused += 1,
                    (typed, keeping) => panic!(
                        "{text}: read as {typed:?}, kept as {:?}",
                        keeping.map(|keeping| keeping.known)
                    ),
                }
            }
        }
        (took, refused)
    }

    /// A group's times and a letter's entry of the tally read with what
    /// they hold that this build does not read read as their own readers
    /// read them, one value at a time: what they take, what they refuse,
    /// and every key they read, given twice, missing, of another type, or
    /// beside keys they do not read, as an object or as a list. Read
    /// directly, they refuse beside a value JSON's numbers or text cannot
    /// hold under a key their own readers pass over, and nothing else.
    #[test]
    fn the_readers_keeping_unknown_keys_read_as_the_typed_ones() {
        for (read, refused) in [
            reads_as_the_typed_reader::<CharTimes>(0x5eed_c4a7),
            reads_as_the_typed_reader::<LetterAccuracy>(0x5eed_1e77),
        ] {
            assert!((15_000..57_000).contains(&read), "{read} of 60,000 read");
            assert!(refused > 100, "{refused} refused beside");
        }
    }

    /// What a value read whole is written with beside its own keys is
    /// written after them, but for a key the value reads: written, it would
    /// be read back as the value's own, or twice, and the value not at all.
    /// With nothing beside them, the value is written as it writes itself.
    #[test]
    fn keys_kept_beside_a_value_are_written_after_its_own_but_for_those_it_reads() {
        let known = LetterAccuracy {
            correct: 3,
            total: 4,
        };
        let none = Map::new();
        let alone = WithKeys {
            known: &known,
            unknown: &none,
        };
        let written = serde_json::to_string(&alone).unwrap();
        assert_eq!(written, serde_json::to_string(&known).unwrap());
        let mut unknown = Map::new();
        unknown.insert("later".into(), Value::from(2));
        unknown.insert("correct".into(), Value::from(9));
        let both = WithKeys {
            known: &known,
            unknown: &unknown,
        };
        let written = serde_json::to_string(&both).unwrap();
        assert_eq!(written, r#"{"correct":3,"total":4,"later":2}"#);
        let back: KeepingUnknown<LetterAccuracy> = serde_json::from_str(&written).unwrap();
        assert_eq!(back.known, known);
        assert_eq!(back.unknown.keys().collect::<Vec<_>>(), ["later"]);
    }
}
