//! The page templates Snowbound offers: OneNote 2010's background art, recreated
//! (`gpu::page::template_picture`), placed where OneNote's templates place it, plain
//! page colours and rule lines.

use onestore::page::{RuleLines, VerticalRule};

/// One background picture of a template: the recreated art's name, and the picture's
/// position and size in points from the page origin as OneNote's template stores them.
/// Art without a size draws at its natural size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Art {
    pub art: &'static str,
    pub position: [f32; 2],
    pub size: Option<[f32; 2]>,
}

/// A template by OneNote's name for it, with the art it puts behind the page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Template {
    pub name: &'static str,
    pub art: &'static [Art],
}

impl Template {
    /// The background pictures a page stores for this template: OneNote's places, our
    /// recreations' bytes.
    #[cfg(feature = "gpu")]
    pub fn pictures(&self) -> Result<Vec<onestore::page::Image>, onestore::page::text::EditError> {
        self.art
            .iter()
            .map(|art| {
                let bytes = crate::gpu::page::template_picture(art.art)
                    .expect("Every template's art is recreated");
                Ok(onestore::page::Image {
                    id: onestore::page::text::new_id()?,
                    layout: onestore::document::Layout {
                        x: Some(art.position[0]),
                        y: Some(art.position[1]),
                        max_width: art.size.map(|size| size[0]),
                        max_height: art.size.map(|size| size[1]),
                        width_set_by_user: art.size.map(|_| true),
                        ..Default::default()
                    },
                    size: art.size,
                    bytes: Some(bytes.into()),
                    alt: None,
                    background: true,
                })
            })
            .collect()
    }
}

/// OneNote 2010's Decorative templates and Informal Meeting Notes (resources/onenote-templates.md),
/// in the order the gallery lists them.
pub const TEMPLATES: &[Template] = &[
    Template {
        name: "Informal Meeting Notes",
        art: &[Art {
            art: "small-push-pins",
            position: [-18.0, -39.6],
            size: Some([610.3, 789.8]),
        }],
    },
    Template {
        name: "Bamboo",
        art: &[Art {
            art: "bamboo",
            position: [-36.0, -75.6],
            size: Some([581.1, 754.3]),
        }],
    },
    Template {
        name: "Binoculars Corner",
        art: &[Art {
            art: "binoculars-corner",
            position: [-54.0, -12.6],
            size: Some([353.7, 80.0]),
        }],
    },
    Template {
        name: "Black and Green Title",
        art: &[Art {
            art: "black-and-green-title",
            position: [0.0, -36.6],
            size: Some([633.7, 39.6]),
        }],
    },
    Template {
        name: "Blue Bubbles Corner",
        art: &[
            Art {
                art: "blue-bubbles-corner-margin",
                position: [-54.0, -3.6],
                size: Some([59.6, 247.5]),
            },
            Art {
                art: "blue-bubbles-corner-top",
                position: [443.2, -3.6],
                size: Some([47.7, 63.7]),
            },
        ],
    },
    Template {
        name: "Blue Clouds",
        art: &[Art {
            art: "blue-clouds",
            position: [-2.2, 0.9],
            size: Some([662.2, 905.0]),
        }],
    },
    Template {
        name: "Blue Dots",
        art: &[Art {
            art: "blue-dots",
            position: [-1.5, 0.1],
            size: Some([602.7, 780.0]),
        }],
    },
    Template {
        name: "Blue Flow",
        art: &[Art {
            art: "blue-flow",
            position: [0.0, -0.6],
            size: Some([33.1, 498.0]),
        }],
    },
    Template {
        name: "Blue Mist Margin",
        art: &[Art {
            art: "blue-mist-margin",
            position: [-26.2, -3.6],
            size: Some([36.0, 431.8]),
        }],
    },
    Template {
        name: "Blue Stripe Title",
        art: &[Art {
            art: "blue-stripe-title",
            position: [0.0, -12.6],
            size: Some([662.2, 103.0]),
        }],
    },
    Template {
        name: "Blue Stripes",
        art: &[Art {
            art: "blue-stripes",
            position: [-63.0, -21.6],
            size: Some([611.2, 791.0]),
        }],
    },
    Template {
        name: "Blue Swirls",
        art: &[Art {
            art: "blue-swirls",
            position: [0.0, -3.6],
            size: Some([595.6, 775.0]),
        }],
    },
    Template {
        name: "Blue Wave Title",
        art: &[Art {
            art: "blue-wave-title",
            position: [-25.5, -15.6],
            size: Some([547.2, 42.9]),
        }],
    },
    Template {
        name: "Blue Waves",
        art: &[Art {
            art: "blue-waves",
            position: [0.0, -12.6],
            size: Some([634.3, 812.5]),
        }],
    },
    Template {
        name: "Books",
        art: &[Art {
            art: "books",
            position: [-72.0, -3.6],
            size: Some([183.9, 658.9]),
        }],
    },
    Template {
        name: "Bouquet",
        art: &[Art {
            art: "bouquet",
            position: [-45.0, -39.6],
            size: Some([505.9, 654.8]),
        }],
    },
    Template {
        name: "Boxes",
        art: &[Art {
            art: "boxes",
            position: [-36.0, -3.6],
            size: None,
        }],
    },
    Template {
        name: "Boxes - Shadow",
        art: &[Art {
            art: "boxes-shadow",
            position: [-18.0, 14.4],
            size: Some([30.0, 456.8]),
        }],
    },
    Template {
        name: "Bubbles",
        art: &[Art {
            art: "bubbles",
            position: [-18.0, -3.6],
            size: Some([36.0, 425.3]),
        }],
    },
    Template {
        name: "Bubbles Margin",
        art: &[
            Art {
                art: "bubbles-margin-margin",
                position: [-54.0, 68.4],
                size: Some([54.9, 288.0]),
            },
            Art {
                art: "bubbles-margin-top",
                position: [-54.0, -21.6],
                size: Some([432.0, 70.4]),
            },
        ],
    },
    Template {
        name: "Buildings",
        art: &[Art {
            art: "buildings",
            position: [-36.0, -21.6],
            size: Some([597.5, 773.3]),
        }],
    },
    Template {
        name: "Chain Title",
        art: &[Art {
            art: "chain-title",
            position: [0.0, -11.1],
            size: Some([533.7, 95.7]),
        }],
    },
    Template {
        name: "Circles",
        art: &[Art {
            art: "circles",
            position: [-54.0, -3.6],
            size: Some([68.7, 549.8]),
        }],
    },
    Template {
        name: "Clock",
        art: &[Art {
            art: "clock",
            position: [-18.0, -3.6],
            size: Some([607.9, 786.7]),
        }],
    },
    Template {
        name: "Columns",
        art: &[Art {
            art: "columns",
            position: [-37.5, -8.9],
            size: Some([602.1, 779.2]),
        }],
    },
    Template {
        name: "Computer",
        art: &[Art {
            art: "computer",
            position: [-18.0, -21.6],
            size: Some([462.7, 618.0]),
        }],
    },
    Template {
        name: "Cyan Title",
        art: &[Art {
            art: "cyan-title",
            position: [0.7, -12.6],
            size: Some([717.0, 102.3]),
        }],
    },
    Template {
        name: "Day Planner",
        art: &[Art {
            art: "day-planner",
            position: [-36.0, -3.6],
            size: Some([482.5, 487.2]),
        }],
    },
    Template {
        name: "Fireworks",
        art: &[Art {
            art: "fireworks",
            position: [-72.0, -3.6],
            size: Some([98.8, 288.0]),
        }],
    },
    Template {
        name: "Flat Squares",
        art: &[Art {
            art: "flat-squares",
            position: [-18.0, -3.6],
            size: Some([30.0, 492.8]),
        }],
    },
    Template {
        name: "Flowers and Hearts",
        art: &[Art {
            art: "flowers-and-hearts",
            position: [-54.0, -3.6],
            size: Some([88.8, 689.8]),
        }],
    },
    Template {
        name: "Glasses Corner",
        art: &[Art {
            art: "glasses-corner",
            position: [-81.0, -56.1],
            size: Some([229.5, 120.7]),
        }],
    },
    Template {
        name: "Globe",
        art: &[Art {
            art: "globe",
            position: [-54.0, -3.6],
            size: Some([142.7, 557.7]),
        }],
    },
    Template {
        name: "Graph",
        art: &[Art {
            art: "graph",
            position: [-36.0, -3.6],
            size: Some([46.5, 495.7]),
        }],
    },
    Template {
        name: "Green Stripes",
        art: &[Art {
            art: "green-stripes",
            position: [-72.0, -21.6],
            size: Some([612.0, 792.0]),
        }],
    },
    Template {
        name: "Hearts",
        art: &[Art {
            art: "hearts",
            position: [-90.0, -3.6],
            size: Some([89.2, 288.0]),
        }],
    },
    Template {
        name: "Ivy",
        art: &[Art {
            art: "ivy",
            position: [-27.0, -3.6],
            size: Some([174.5, 640.0]),
        }],
    },
    Template {
        name: "Large Push Pins",
        art: &[Art {
            art: "large-push-pins",
            position: [-18.0, -21.6],
            size: Some([529.7, 685.5]),
        }],
    },
    Template {
        name: "Light Bulb Corner",
        art: &[Art {
            art: "light-bulb-corner",
            position: [0.0, -5.9],
            size: Some([155.7, 164.7]),
        }],
    },
    Template {
        name: "Lilies",
        art: &[Art {
            art: "lilies",
            position: [0.0, -3.6],
            size: Some([618.5, 811.5]),
        }],
    },
    Template {
        name: "Math",
        art: &[Art {
            art: "math",
            position: [-36.0, -21.6],
            size: Some([512.6, 568.1]),
        }],
    },
    Template {
        name: "Networking",
        art: &[
            Art {
                art: "networking",
                position: [-36.0, 212.4],
                size: None,
            },
            Art {
                art: "networking",
                position: [-36.0, 104.4],
                size: None,
            },
            Art {
                art: "networking",
                position: [-36.0, -3.6],
                size: None,
            },
        ],
    },
    Template {
        name: "Notebook",
        art: &[Art {
            art: "notebook",
            position: [-18.0, -21.6],
            size: Some([483.9, 626.3]),
        }],
    },
    Template {
        name: "Orange Margin",
        art: &[Art {
            art: "orange-margin",
            position: [-27.0, -3.6],
            size: Some([37.4, 483.0]),
        }],
    },
    Template {
        name: "Paper",
        art: &[Art {
            art: "paper",
            position: [-18.0, -57.6],
            size: Some([612.6, 792.7]),
        }],
    },
    Template {
        name: "Paws - Cyan",
        art: &[Art {
            art: "paws-cyan",
            position: [-18.0, -3.6],
            size: Some([36.0, 288.0]),
        }],
    },
    Template {
        name: "Pencil and Notebook",
        art: &[Art {
            art: "pencil-and-notebook",
            position: [-18.0, -57.6],
            size: Some([620.8, 469.5]),
        }],
    },
    Template {
        name: "Pens",
        art: &[Art {
            art: "pens",
            position: [-36.0, -39.6],
            size: Some([601.0, 777.8]),
        }],
    },
    Template {
        name: "Pixel",
        art: &[Art {
            art: "pixel",
            position: [-18.0, -21.6],
            size: Some([432.0, 26.3]),
        }],
    },
    Template {
        name: "Pixel - Blends",
        art: &[Art {
            art: "pixel-blends",
            position: [-18.0, -3.6],
            size: Some([29.2, 354.0]),
        }],
    },
    Template {
        name: "Plain and Simple",
        art: &[Art {
            art: "plain-and-simple",
            position: [-36.0, -12.6],
            size: Some([606.8, 785.3]),
        }],
    },
    Template {
        name: "Purple Clouds",
        art: &[Art {
            art: "purple-clouds",
            position: [0.0, -21.6],
            size: Some([613.5, 163.7]),
        }],
    },
    Template {
        name: "Pushpins Corner",
        art: &[Art {
            art: "pushpins-corner",
            position: [-36.0, -39.6],
            size: Some([162.0, 111.9]),
        }],
    },
    Template {
        name: "Rainbow",
        art: &[Art {
            art: "rainbow",
            position: [-36.0, -3.6],
            size: Some([612.0, 792.0]),
        }],
    },
    Template {
        name: "Rainbow Border",
        art: &[Art {
            art: "rainbow-border",
            position: [-18.0, -3.6],
            size: Some([29.2, 399.0]),
        }],
    },
    Template {
        name: "Rainbow Mini Border",
        art: &[Art {
            art: "rainbow-mini-border",
            position: [-18.0, -3.6],
            size: Some([24.0, 368.2]),
        }],
    },
    Template {
        name: "Recipe",
        art: &[Art {
            art: "recipe",
            position: [-45.0, -3.6],
            size: Some([533.2, 690.0]),
        }],
    },
    Template {
        name: "Red and Black Margin",
        art: &[Art {
            art: "red-and-black-margin",
            position: [0.7, 6.9],
            size: Some([11.8, 484.5]),
        }],
    },
    Template {
        name: "Side Stripes",
        art: &[Art {
            art: "side-stripes",
            position: [-36.0, -3.6],
            size: Some([40.2, 848.4]),
        }],
    },
    Template {
        name: "Small Push Pins",
        art: &[Art {
            art: "small-push-pins",
            position: [-36.0, -39.6],
            size: Some([610.3, 789.8]),
        }],
    },
    Template {
        name: "Sparks",
        art: &[Art {
            art: "sparks",
            position: [-18.0, -3.6],
            size: Some([611.4, 791.3]),
        }],
    },
    Template {
        name: "Stars",
        art: &[Art {
            art: "stars",
            position: [-36.0, -3.6],
            size: Some([61.8, 341.9]),
        }],
    },
    Template {
        name: "Swirls",
        art: &[Art {
            art: "swirls",
            position: [-36.0, -3.6],
            size: Some([56.2, 366.3]),
        }],
    },
    Template {
        name: "Tiles",
        art: &[Art {
            art: "tiles",
            position: [-36.0, -3.6],
            size: Some([606.2, 784.5]),
        }],
    },
    Template {
        name: "Triangles Title",
        art: &[
            Art {
                art: "triangles-title-top",
                position: [0.0, -21.6],
                size: Some([274.2, 53.0]),
            },
            Art {
                art: "triangles-title-margin",
                position: [5.2, 231.9],
                size: Some([11.8, 288.0]),
            },
        ],
    },
    Template {
        name: "Tulips",
        art: &[Art {
            art: "tulips",
            position: [-72.0, -3.6],
            size: Some([605.0, 783.0]),
        }],
    },
    Template {
        name: "Window Reflection",
        art: &[Art {
            art: "window-reflection",
            position: [-18.0, -75.6],
            size: Some([603.3, 780.7]),
        }],
    },
    Template {
        name: "Writing",
        art: &[Art {
            art: "writing",
            position: [-54.0, -39.6],
            size: Some([597.5, 773.2]),
        }],
    },
];

/// OneNote 2010's page colours in its View, Page Color menu order, with the names its
/// tooltips give them; COLORREF, as the page XML reports each
/// (`corpus/notebook-management/native/page-color`).
pub const PAGE_COLORS: &[(&str, u32)] = &[
    ("Blue", 0xfef5ed),
    ("Red", 0xefeeff),
    ("Lemon", 0xddfdfd),
    ("Apple", 0xe5f9ec),
    ("Blue Mist", 0xf3ede5),
    ("Magenta", 0xf6edfb),
    ("Yellow", 0xdffaff),
    ("Green", 0xe5f0ec),
    ("Silver", 0xede9e9),
    ("Purple", 0xffe9f3),
    ("Tan", 0xe7f4f9),
    ("Cyan", 0xebefe5),
    ("Purple Mist", 0xebe3e8),
    ("Red Chalk", 0xdedeef),
    ("Orange", 0xe1f2fb),
    ("Teal", 0xf2f9d4),
];

/// OneNote 2010's View, Rule Lines presets in its menu order, in its default light blue
/// with a red margin line
/// (`corpus/rule-lines/native`).
pub const RULE_LINES: &[(&str, RuleLines)] = &[
    ("Narrow Ruled", ruled(0x3ebe_f8d2)),
    ("College Ruled", ruled(0x3f28_f5c3)),
    ("Standard Ruled", ruled(0x3f6b_851f)),
    ("Wide Ruled", ruled(0x3fa6_6666)),
    ("Small Grid", grid(0x3eaa_aaab)),
    ("Medium Grid", grid(0x3f49_930c)),
    ("Large Grid", grid(0x3f97_2e49)),
    ("Very Large Grid", grid(0x3fc9_930c)),
];

const RULE_COLOR: u32 = 0xfdebca;

/// Lines `spacing` apart, the bits of the half inches OneNote stores.
const fn ruled(spacing: u32) -> RuleLines {
    RuleLines {
        spacing: f32::from_bits(spacing),
        color: RULE_COLOR,
        vertical: VerticalRule::Margin(0x5050ff),
    }
}

const fn grid(spacing: u32) -> RuleLines {
    RuleLines {
        spacing: f32::from_bits(spacing),
        color: RULE_COLOR,
        vertical: VerticalRule::Grid {
            spacing: f32::from_bits(spacing),
            color: RULE_COLOR,
        },
    }
}

/// The template named `name`.
pub fn find(name: &str) -> Option<&'static Template> {
    TEMPLATES.iter().find(|template| template.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_have_unique_names_and_art() {
        let mut names: Vec<_> = TEMPLATES.iter().map(|template| template.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), TEMPLATES.len());
        assert!(TEMPLATES.iter().all(|template| !template.art.is_empty()));
        assert_eq!(find("Ivy").unwrap().art[0].art, "ivy");
    }
}
