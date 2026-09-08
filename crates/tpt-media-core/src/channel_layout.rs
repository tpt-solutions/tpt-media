//! Channel layout for audio frames.

/// Bitmask identifying a set of audio channels.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct ChannelMask(u16);

impl ChannelMask {
    pub const FRONT_LEFT: ChannelMask = ChannelMask(0x0001);
    pub const FRONT_RIGHT: ChannelMask = ChannelMask(0x0002);
    pub const FRONT_CENTER: ChannelMask = ChannelMask(0x0004);
    pub const LOW_FREQUENCY: ChannelMask = ChannelMask(0x0008);
    pub const BACK_LEFT: ChannelMask = ChannelMask(0x0010);
    pub const BACK_RIGHT: ChannelMask = ChannelMask(0x0020);

    /// Empty mask (no channels).
    pub const EMPTY: ChannelMask = ChannelMask(0);

    /// Raw bit value.
    pub const fn bits(self) -> u16 {
        self.0
    }

    /// Build a mask from raw channel bits (used by deserializers).
    pub const fn from_bits(bits: u16) -> ChannelMask {
        ChannelMask(bits)
    }

    /// `true` if any channel is set.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// `true` if this mask contains every bit of `other`.
    pub const fn contains(self, other: ChannelMask) -> bool {
        (self.0 & other.0) == other.0
    }

    /// Number of channels in the mask (counts set bits).
    pub fn channels(self) -> u8 {
        self.0.count_ones() as u8
    }
}

impl core::ops::BitOr for ChannelMask {
    type Output = ChannelMask;
    fn bitor(self, rhs: ChannelMask) -> ChannelMask {
        ChannelMask(self.0 | rhs.0)
    }
}

impl core::ops::BitOrAssign for ChannelMask {
    fn bitor_assign(&mut self, rhs: ChannelMask) {
        self.0 |= rhs.0;
    }
}

/// Named channel layouts.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ChannelLayout {
    /// Single channel.
    Mono,
    /// Two channels: front-left, front-right.
    Stereo,
    /// 5.1 surround: FL, FR, FC, LFE, BL, BR.
    Surround51,
    /// Custom mask.
    Custom(ChannelMask),
}

impl ChannelLayout {
    /// `ChannelLayout::Mono`.
    pub const MONO: ChannelLayout = ChannelLayout::Mono;
    /// `ChannelLayout::Stereo`.
    pub const STEREO: ChannelLayout = ChannelLayout::Stereo;

    /// Number of channels in this layout.
    pub fn channels(self) -> u8 {
        match self {
            ChannelLayout::Mono => 1,
            ChannelLayout::Stereo => 2,
            ChannelLayout::Surround51 => 6,
            ChannelLayout::Custom(m) => m.channels(),
        }
    }

    /// Underlying channel mask.
    pub fn mask(self) -> ChannelMask {
        match self {
            ChannelLayout::Mono => ChannelMask(0x0004), // FC
            ChannelLayout::Stereo => ChannelMask::FRONT_LEFT | ChannelMask::FRONT_RIGHT,
            ChannelLayout::Surround51 => {
                ChannelMask::FRONT_LEFT
                    | ChannelMask::FRONT_RIGHT
                    | ChannelMask::FRONT_CENTER
                    | ChannelMask::LOW_FREQUENCY
                    | ChannelMask::BACK_LEFT
                    | ChannelMask::BACK_RIGHT
            }
            ChannelLayout::Custom(m) => m,
        }
    }

    /// Layout from a channel count.
    pub fn from_channels(channels: u8) -> Self {
        match channels {
            1 => ChannelLayout::Mono,
            2 => ChannelLayout::Stereo,
            6 => ChannelLayout::Surround51,
            n => ChannelLayout::Custom(ChannelMask((1u16 << n).wrapping_sub(1))),
        }
    }
}

impl Default for ChannelLayout {
    /// Default to `Stereo`, *not* `Mono` — `#[derive(Default)]` would
    /// select the first variant (`Mono`), which is the wrong default for
    /// a media framework where a freshly-constructed track with no
    /// explicit layout is more likely to be stereo than mono.
    #[allow(clippy::derivable_impls)]
    fn default() -> Self {
        ChannelLayout::Stereo
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_is_one_channel() {
        assert_eq!(ChannelLayout::Mono.channels(), 1);
    }

    #[test]
    fn stereo_channels() {
        assert_eq!(ChannelLayout::Stereo.channels(), 2);
    }

    #[test]
    fn surround_channels() {
        assert_eq!(ChannelLayout::Surround51.channels(), 6);
    }

    #[test]
    fn mask_combine() {
        let m = ChannelMask::FRONT_LEFT | ChannelMask::FRONT_RIGHT;
        assert_eq!(m.channels(), 2);
        assert!(m.contains(ChannelMask::FRONT_LEFT));
    }

    #[test]
    fn from_channels() {
        assert_eq!(ChannelLayout::from_channels(1), ChannelLayout::Mono);
        assert_eq!(ChannelLayout::from_channels(2), ChannelLayout::Stereo);
    }
}
