use super::MapTheme;
use egui::Color32;

impl MapTheme {
    /// Boundary colors in OSM level order, from countries (2) to local areas (11).
    pub fn admin_palette(self) -> [Color32; 10] {
        let rgb: [u32; 10] = match self {
            Self::Topo => [
                0xffdc83, 0xf79460, 0x61d8ee, 0x83a9e4, 0xe991ad, 0xb9a4dd, 0x89deb3, 0x50b7b1,
                0xe3ad87, 0xbbd0df,
            ],
            Self::Phosphor => [
                0xeeffae, 0x90c74d, 0x55ed91, 0x7abb9b, 0xffc961, 0xc7e77b, 0x40bfa9, 0x9febbc,
                0xc7a775, 0xb2c6a1,
            ],
            Self::Thermal => [
                0xffe49a, 0xeea455, 0xc493ff, 0x929fef, 0xf47da9, 0xe7b9ee, 0xf6b773, 0xaf85bd,
                0xe08372, 0xd1cbe7,
            ],
            Self::Ghost => [
                0xfafafa, 0xc8c8c8, 0xdfdfdf, 0x9f9f9f, 0xbbbbbb, 0xededed, 0x939393, 0xd3d3d3,
                0x828282, 0xadadad,
            ],
            Self::Akira => [
                0xffdfdc, 0xf9827b, 0x4de4ff, 0x729bfa, 0xff586e, 0xd397e6, 0x9bcaf1, 0x57b9ba,
                0xe793ad, 0xbac5d1,
            ],
            Self::Sodium => [
                0xfff0cb, 0xd59649, 0xffc454, 0xb9835b, 0xf38d42, 0xe8cda5, 0xc1a372, 0xf8d878,
                0xb97c36, 0xeaaa76,
            ],
            Self::Lunar => [
                0xf3eddd, 0xabb4c1, 0xb4d2e4, 0x9299af, 0xd6c2a3, 0xd5d9e6, 0x9caaa1, 0xc6bbcb,
                0xb09f93, 0x8b95a0,
            ],
            Self::Mars => [
                0xffe4b5, 0xe6a464, 0xf19b77, 0xbb8d79, 0xffc47b, 0xe1bda9, 0xd67e71, 0xcead80,
                0xe99694, 0xc5b4a5,
            ],
            Self::MarsDark => [
                0xffd6ce, 0xd98178, 0xff6169, 0xb68a93, 0xffac89, 0xec99ae, 0xd7788e, 0xc9b2ad,
                0xf58969, 0xa297a6,
            ],
        };
        rgb.map(|hex| {
            let [_, red, green, blue] = hex.to_be_bytes();
            Color32::from_rgb(red, green, blue)
        })
    }
}
