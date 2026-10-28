//! PdfCraft patch (72): overprint (ISO 32000-2 §8.6.7, §11.7.4.5) in a `DeviceCMYK` blending
//! space: what one paint does to each process ink and to the spot inks beside them, after the
//! `CompatibleOverprint` table (ISO 32000-1 §11.7.4.3, Table 149).
//!
//! Adapted from pdfcer (MIT, same author), `pdfcer-render/src/overprint.rs`.

use crate::color::{ColorSpace, ColorSpaceType};
use smallvec::SmallVec;

/// What a paint does to one ink.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ink {
    /// Paints this tint (0 = no ink, so `Paint(0.0)` knocks the ink out).
    Paint(f32),
    /// Leaves what is already there.
    Keep,
}

/// `OPM` (§8.6.7): whether a zero `DeviceCMYK` component leaves the ink under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverprintMode {
    /// `OPM 0`, and any value other than 1.
    #[default]
    Zero,
    /// `OPM 1`.
    One,
}

/// A spot ink: a colorant that is not a process one, and how to show it.
#[derive(Debug, Clone)]
pub struct Spot {
    /// The colorant name.
    pub name: Vec<u8>,
    space: ColorSpace,
    index: usize,
}

impl Spot {
    /// The colour of this ink alone at `tint`, through its space's alternate, in sRGB.
    pub fn srgb(&self, tint: f32) -> [f32; 3] {
        let n = usize::from(self.space.num_components().max(1));
        let mut c: SmallVec<[f32; 8]> = SmallVec::from_elem(0.0, n);
        if let Some(slot) = c.get_mut(self.index) {
            *slot = tint.clamp(0.0, 1.0);
        }
        let [r, g, b, _] = self.space.to_rgba(&c, 1.0, false).components();
        [r, g, b]
    }
}

/// What a paint does to every ink of the blending space.
#[derive(Debug, Clone)]
pub struct InkPaint {
    /// Cyan, magenta, yellow and black.
    pub process: [Ink; 4],
    /// The spot inks the paint names, with their tints.
    pub spots: SmallVec<[(Spot, f32); 2]>,
    /// What every other spot ink gets: kept under overprint, knocked out otherwise.
    pub other_spots: Ink,
}

impl InkPaint {
    /// What the paint does to spot ink `name`.
    pub fn spot(&self, name: &[u8]) -> Ink {
        self.spots
            .iter()
            .find(|(s, _)| s.name == name)
            .map_or(self.other_spots, |(_, t)| Ink::Paint(*t))
    }
}

const PROCESS: [&[u8]; 4] = [b"Cyan", b"Magenta", b"Yellow", b"Black"];

/// The inks colour `c` of `space` paints, with overprint `overprint` (`None` when off).
/// `process` gives a colour's process tints when it has no colorants of its own. `None` when
/// the colour has no inks.
pub(crate) fn ink_paint(
    space: &ColorSpace,
    c: &[f32],
    overprint: Option<OverprintMode>,
    process: impl Fn() -> Option<[f32; 4]>,
) -> Option<InkPaint> {
    // PdfCraft patch (72): an indexed colour paints its base colour's inks.
    if let ColorSpaceType::Indexed(i) = space.kind() {
        let base = i.lookup(c.first().copied().unwrap_or(0.0))?;
        return ink_paint(i.base(), base, overprint, process);
    }
    let get = |i: usize| c.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
    let unnamed = if overprint.is_some() { Ink::Keep } else { Ink::Paint(0.0) };
    let mut paint = InkPaint {
        process: [Ink::Paint(0.0); 4],
        spots: SmallVec::new(),
        other_spots: unnamed,
    };
    let colorants: SmallVec<[&[u8]; 4]> = match space.kind() {
        ColorSpaceType::DeviceCmyk => {
            let tints = [get(0), get(1), get(2), get(3)];
            // Table 149: only a direct DeviceCMYK paint under OPM 1 keeps an ink it has none of.
            let keep_zero = overprint == Some(OverprintMode::One);
            paint.process = tints.map(|t| if keep_zero && t == 0.0 { Ink::Keep } else { Ink::Paint(t) });
            return Some(paint);
        }
        ColorSpaceType::Separation(s) if s.is_all => {
            paint.process = [Ink::Paint(get(0)); 4];
            paint.other_spots = Ink::Paint(get(0));
            return Some(paint);
        }
        ColorSpaceType::Separation(s) => SmallVec::from_iter([s.name.as_slice()]),
        ColorSpaceType::DeviceN(d) => d.names.iter().map(Vec::as_slice).collect(),
        _ => {
            paint.process = process()?.map(Ink::Paint);
            return Some(paint);
        }
    };
    // A Separation or DeviceN paint: the process inks it names take their tints, the others
    // are kept under overprint and knocked out without it, as are the spot inks it doesn't name.
    paint.process = [unnamed; 4];
    for (index, name) in colorants.iter().enumerate() {
        let tint = get(index);
        if let Some(i) = PROCESS.iter().position(|p| p == name) {
            if let Some(slot) = paint.process.get_mut(i) {
                *slot = Ink::Paint(tint);
            }
        } else if *name != b"None" && !paint.spots.iter().any(|(s, _)| s.name == *name) {
            let spot = Spot { name: name.to_vec(), space: space.clone(), index };
            paint.spots.push((spot, tint));
        }
    }
    Some(paint)
}


/// PdfCraft patch (72): one ink of a `DeviceCMYK` blending space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Channel {
    /// Cyan, magenta, yellow or black (0..=3).
    Process(usize),
    /// A spot ink, by colorant name.
    Spot(Vec<u8>),
}

/// PdfCraft patch (72): where an image's samples get one ink's tint from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelSource {
    /// The image's component at this index.
    Component(usize),
    /// The image's colour converted to process tints (it has no colorants of its own).
    Converted,
    /// The image leaves this ink alone (overprint).
    Keep,
    /// The image knocks this ink out.
    Zero,
}

/// Where an image in `space` painted with overprint `overprint` gets `channel`'s tint from.
/// An image is never subject to `OPM 1` (ISO 32000-2 §8.6.7: it applies to `DeviceCMYK` paints
/// only), so overprint keeps exactly the inks its space doesn't name.
pub(crate) fn channel_source(space: Option<&ColorSpace>, channel: &Channel, overprint: bool) -> ChannelSource {
    let unnamed = if overprint { ChannelSource::Keep } else { ChannelSource::Zero };
    let named = |names: &[&[u8]]| {
        let wanted: &[u8] = match channel {
            Channel::Process(i) => PROCESS.get(*i).copied().unwrap_or_default(),
            Channel::Spot(name) => name,
        };
        names.iter().position(|n| *n == wanted).map_or(unnamed, ChannelSource::Component)
    };
    match space.map(ColorSpace::kind) {
        // An indexed image's samples are base colours.
        Some(ColorSpaceType::Indexed(i)) => channel_source(Some(i.base()), channel, overprint),
        Some(ColorSpaceType::DeviceCmyk) => match channel {
            Channel::Process(i) if *i < 4 => ChannelSource::Component(*i),
            _ => unnamed,
        },
        Some(ColorSpaceType::Separation(s)) if s.is_all => ChannelSource::Component(0),
        Some(ColorSpaceType::Separation(s)) => named(&[s.name.as_slice()]),
        Some(ColorSpaceType::DeviceN(d)) => {
            let names: SmallVec<[&[u8]; 8]> = d.names.iter().map(Vec::as_slice).collect();
            named(&names)
        }
        _ => match channel {
            Channel::Process(_) => ChannelSource::Converted,
            Channel::Spot(_) => unnamed,
        },
    }
}

/// The spot inks `space` names, for an image or shading in it.
pub(crate) fn spots_of(space: &ColorSpace) -> SmallVec<[Spot; 2]> {
    if let ColorSpaceType::Indexed(i) = space.kind() {
        return spots_of(i.base());
    }
    let initial = space.initial_color();
    ink_paint(space, &initial, None, || Some([0.0; 4]))
        .map(|p| p.spots.into_iter().map(|(s, _)| s).collect())
        .unwrap_or_default()
}
