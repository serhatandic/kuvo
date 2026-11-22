//! The Library as a stand-in box answers it (`kuvo/library`): eighty things handed over across five
//! bots, your own sessions and two computers, each picture and page with a thumbnail drawn here as a
//! PNG (as a box's would come), and removing one.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Sender;

use image::RgbaImage;
use serde_json::{Value, json};

/// The bots the Library's shots show, with their looks.
pub const BOTS: [(&str, &str, &str, &str); 5] = [
    ("bot_foreman", "Foreman", "indigo", "squircle"),
    ("bot_mira", "Mira", "coral", "bloom"),
    ("bot_scout", "Scout", "sky", "pebble"),
    ("bot_scribe", "Scribe", "violet", "round"),
    ("bot_builder", "Builder", "mint", "gem"),
];

#[derive(Clone, Copy, PartialEq)]
enum Motif {
    Sunset,
    Botanical,
    UiLight,
    UiDark,
    ChartLine,
    ChartBars,
    Diagram,
    Warm,
    Cool,
    Phone,
    Icons,
    Studio,
    Pdf,
    Page,
    None,
}

/// One thing handed over: its id, name, whose (a bot, or yours), computer, how long ago in minutes,
/// and how its thumbnail is drawn, with a hue.
type Row = (&'static str, &'static str, Option<&'static str>, bool, f64, Motif, f32);

const H: f64 = 60.;
const D: f64 = 24. * 60.;

#[rustfmt::skip]
const ROWS: &[Row] = &[
    ("lib_01", "Hero Dusk Coastline.png", Some("bot_mira"), true, 14., Motif::Sunset, 24.),
    ("lib_02", "Q4 Board Brief.pdf", Some("bot_scribe"), false, 38., Motif::Pdf, 218.),
    ("lib_03", "Pricing Calculator.html", Some("bot_builder"), false, H, Motif::Page, 210.),
    ("lib_04", "Dark Sidebar Hierarchy.png", Some("bot_foreman"), true, 2. * H, Motif::UiDark, 232.),
    ("lib_05", "Fleet Latency Breakdown.png", Some("bot_scout"), false, 2. * H + 5., Motif::ChartLine, 198.),
    ("lib_06", "Series A Term Sheet.pdf", None, true, 3. * H, Motif::Pdf, 260.),
    ("lib_07", "Bot Avatar Grid v4.png", Some("bot_mira"), true, 4. * H, Motif::Icons, 32.),
    ("lib_08", "Release 0.8 Notes.html", Some("bot_scribe"), false, 5. * H, Motif::Page, 162.),
    ("lib_09", "iOS Onboarding Flow.png", Some("bot_builder"), true, 5. * H + 10., Motif::Phone, 215.),
    ("lib_10", "Relay Failover Diagram.png", Some("bot_foreman"), false, 6. * H, Motif::Diagram, 245.),
    ("lib_11", "November Cloud Invoice.pdf", Some("bot_scout"), false, 7. * H, Motif::Pdf, 148.),
    ("lib_12", "Botanical Desk Study.png", None, true, 8. * H, Motif::Botanical, 142.),
    ("lib_13", "Churn Cohort Nov.csv", Some("bot_scout"), false, 9. * H, Motif::None, 170.),
    ("lib_14", "Settings Glass Light.png", Some("bot_mira"), true, 10. * H, Motif::UiLight, 206.),
    ("lib_15", "Product Pitch Deck.pdf", Some("bot_scribe"), true, D + 2. * H, Motif::Pdf, 18.),
    ("lib_16", "Latency Explorer.html", Some("bot_scout"), false, D + 3. * H, Motif::Page, 200.),
    ("lib_17", "Warm Terracotta Mesh.png", Some("bot_mira"), true, D + 4. * H, Motif::Warm, 16.),
    ("lib_18", "Payment Dispute Card.png", None, true, D + 5. * H, Motif::UiLight, 345.),
    ("lib_19", "SOC2 Type II Summary.pdf", Some("bot_foreman"), false, D + 6. * H, Motif::Pdf, 224.),
    ("lib_20", "Session Tree Mockup.png", Some("bot_builder"), true, D + 7. * H, Motif::UiDark, 255.),
    ("lib_21", "Weekly Token Spend.png", Some("bot_scout"), false, D + 8. * H, Motif::ChartBars, 164.),
    ("lib_22", "Customer Quote Cards.png", Some("bot_mira"), true, D + 9. * H, Motif::Cool, 228.),
    ("lib_23", "Vendor MSA Redline.pdf", None, true, D + 10. * H, Motif::Pdf, 280.),
    ("lib_24", "Onboarding Demo.html", Some("bot_builder"), false, D + 11. * H, Motif::Page, 240.),
    ("lib_25", "Studio Desk Setup.jpg", None, true, D + 12. * H, Motif::Studio, 38.),
    ("lib_26", "Brand Kit Export.zip", Some("bot_mira"), true, D + 13. * H, Motif::None, 28.),
    ("lib_27", "Queue Triage Flow.png", Some("bot_foreman"), false, D + 14. * H, Motif::Diagram, 268.),
    ("lib_28", "October Receipt Bundle.pdf", None, true, D + 15. * H, Motif::Pdf, 192.),
    ("lib_29", "iOS Goal Dashboard.png", Some("bot_builder"), true, 2. * D, Motif::Phone, 165.),
    ("lib_30", "Enterprise Pricing One-Pager.pdf", Some("bot_scribe"), false, 2. * D + H, Motif::Pdf, 212.),
    ("lib_31", "Alpine Dawn Gradient.png", Some("bot_mira"), true, 2. * D + 2. * H, Motif::Sunset, 290.),
    ("lib_32", "Sync Protocol Overview.png", Some("bot_foreman"), false, 2. * D + 3. * H, Motif::Diagram, 188.),
    ("lib_33", "Usage Insights Spec.pdf", Some("bot_foreman"), false, 3. * D, Motif::Pdf, 248.),
    ("lib_34", "API Benchmarks.xlsx", Some("bot_scout"), false, 3. * D + H, Motif::None, 215.),
    ("lib_35", "Metrics Board Preview.html", Some("bot_scout"), false, 3. * D + 2. * H, Motif::Page, 152.),
    ("lib_36", "Minimal Checklist iOS.png", Some("bot_mira"), true, 3. * D + 3. * H, Motif::Phone, 25.),
    ("lib_37", "Design Review Notes.m4a", None, true, 3. * D + 4. * H, Motif::None, 310.),
    ("lib_38", "Cache Hit Rate Trend.png", Some("bot_scout"), false, 3. * D + 5. * H, Motif::ChartLine, 146.),
    ("lib_39", "Annual Plan Invoice.pdf", None, true, 4. * D, Motif::Pdf, 206.),
    ("lib_40", "Bot Expression Sheet.png", Some("bot_mira"), true, 4. * D + H, Motif::Icons, 182.),
    ("lib_41", "Side Panel Wireframe.png", Some("bot_builder"), true, 4. * D + 2. * H, Motif::UiLight, 222.),
    ("lib_42", "Launch Checklist Page.html", Some("bot_foreman"), false, 4. * D + 3. * H, Motif::Page, 274.),
    ("lib_43", "Customer Research Synthesis.pdf", Some("bot_scribe"), false, 4. * D + 4. * H, Motif::Pdf, 336.),
    ("lib_44", "Nordic Fern Print.png", Some("bot_mira"), true, 5. * D, Motif::Botanical, 156.),
    ("lib_45", "Session Controller Blur.png", None, true, 5. * D + H, Motif::UiDark, 210.),
    ("lib_46", "Provider Latency Bars.png", Some("bot_scout"), false, 5. * D + 2. * H, Motif::ChartBars, 240.),
    ("lib_47", "Security Whitepaper.pdf", Some("bot_scribe"), false, 5. * D + 3. * H, Motif::Pdf, 198.),
    ("lib_48", "Interactive Roadmap.html", Some("bot_foreman"), false, 6. * D, Motif::Page, 178.),
    ("lib_49", "Team Offsite Deck.key", None, true, 6. * D + H, Motif::None, 262.),
    ("lib_50", "Mac Installer Signing.png", Some("bot_builder"), false, 6. * D + 2. * H, Motif::Diagram, 140.),
    ("lib_51", "Cobalt Glass Wave.png", Some("bot_mira"), true, 8. * D, Motif::Cool, 214.),
    ("lib_52", "Q3 Financial Review.pdf", None, true, 8. * D + H, Motif::Pdf, 180.),
    ("lib_53", "Invoice Summary Page.html", Some("bot_builder"), false, 9. * D, Motif::Page, 134.),
    ("lib_54", "Model Picker Matrix.png", Some("bot_builder"), true, 9. * D + H, Motif::UiLight, 246.),
    ("lib_55", "Partner Agreement.pdf", None, true, 10. * D, Motif::Pdf, 22.),
    ("lib_56", "Sunset Kyoto Print.png", None, true, 10. * D + H, Motif::Sunset, 12.),
    ("lib_57", "Daily Active Bots.png", Some("bot_scout"), false, 11. * D, Motif::ChartLine, 264.),
    ("lib_58", "Support Playbook.pdf", Some("bot_scribe"), false, 11. * D + H, Motif::Pdf, 168.),
    ("lib_59", "Hardware Lease Receipt.pdf", None, true, 12. * D, Motif::Pdf, 242.),
    ("lib_60", "App Icon Candidates.png", Some("bot_mira"), true, 12. * D + H, Motif::Icons, 298.),
    ("lib_61", "Fleet Status Prototype.html", Some("bot_builder"), false, 13. * D, Motif::Page, 224.),
    ("lib_62", "Onboarding Copy Deck.pdf", Some("bot_scribe"), true, 13. * D + H, Motif::Pdf, 46.),
    ("lib_63", "Workspace Split Dark.png", Some("bot_foreman"), true, 14. * D, Motif::UiDark, 194.),
    ("lib_64", "Migration Cohort.csv", Some("bot_scout"), false, 14. * D + H, Motif::None, 150.),
    ("lib_65", "Warm Ochre Study.png", Some("bot_mira"), true, 15. * D, Motif::Warm, 42.),
    ("lib_66", "Privacy Architecture.pdf", Some("bot_foreman"), false, 15. * D + H, Motif::Pdf, 232.),
    ("lib_67", "iPhone Pairing Sheet.png", Some("bot_builder"), true, 16. * D, Motif::Phone, 278.),
    ("lib_68", "Office Floorplan.pdf", None, true, 16. * D + H, Motif::Pdf, 162.),
    ("lib_69", "Weekly Digest Template.html", Some("bot_scribe"), false, 17. * D, Motif::Page, 208.),
    ("lib_70", "Server Rack Wiring.jpg", None, false, 17. * D + H, Motif::Studio, 212.),
    ("lib_71", "Token Breakdown Bars.png", Some("bot_scout"), false, 18. * D, Motif::ChartBars, 26.),
    ("lib_72", "Contractor Invoice #42.pdf", None, true, 18. * D + H, Motif::Pdf, 174.),
    ("lib_73", "Press Assets Pack.zip", Some("bot_mira"), true, 19. * D, Motif::None, 202.),
    ("lib_74", "Agent Hand-off Graph.png", Some("bot_foreman"), false, 19. * D + H, Motif::Diagram, 318.),
    ("lib_75", "Beta User Survey.pdf", Some("bot_scribe"), false, 20. * D, Motif::Pdf, 256.),
    ("lib_76", "Eucalyptus Leaf Card.png", Some("bot_mira"), true, 21. * D, Motif::Botanical, 166.),
    ("lib_77", "Founder Standup.m4a", None, true, 22. * D, Motif::None, 230.),
    ("lib_78", "Quarterly Tax Filing.pdf", None, true, 23. * D, Motif::Pdf, 200.),
    ("lib_79", "Sign-in Flow Deck.pdf", Some("bot_builder"), false, 24. * D, Motif::Pdf, 268.),
    ("lib_80", "Docs Landing Preview.html", Some("bot_scribe"), false, 25. * D, Motif::Page, 192.),
];

#[derive(Default)]
pub struct Library {
    /// Removed from the Library (`…/hide`).
    pub hidden: HashSet<String>,
    /// Nothing handed over yet, on one computer.
    pub empty: bool,
    /// Those following it, by their query.
    pub followers: HashMap<u64, (String, Sender<String>)>,
}

fn kind_of(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or("") {
        "png" | "jpg" => "image",
        "pdf" => "document",
        "html" => "page",
        _ => "other",
    }
}

/// A page is named by its title, as a box names it.
fn shown_name(name: &str) -> &str {
    name.strip_suffix(".html").unwrap_or(name)
}

fn item(row: &Row, now: f64) -> Value {
    let (id, name, bot, here, ago, motif, _) = *row;
    let kind = kind_of(name);
    let computer = if here { json!({ "key": "key_here", "name": "MacBook Pro", "here": true }) } else { json!({ "key": "key_studio", "name": "Mac Studio" }) };
    let mut item = json!({
        "id": id, "name": shown_name(name), "kind": kind,
        "type": match kind { "image" => "image/png", "document" => "application/pdf", "page" => "text/html", _ => "application/octet-stream" },
        "how": if bot.is_none() { "attached" } else if kind == "image" { "generated" } else { "shown" },
        "path": format!("/Users/sample/Library Items/{name}"),
        "session": format!("ses_{}", bot.map_or("yours", |bot| bot.trim_start_matches("bot_"))),
        "message": format!("msg_{id}"),
        "at": (now - ago * 60_000.) / 1000., "there": true, "thumbnail": motif != Motif::None,
        "computer": computer,
    });
    if let Some(bot) = bot {
        item["bot"] = json!(bot);
    }
    item
}

impl Library {
    fn list(&self, query: &dyn Fn(&str) -> Option<String>, now: f64) -> Value {
        let source = query("source");
        let kind = query("kind");
        let words: Vec<String> = query("q").unwrap_or_default().to_lowercase().split_whitespace().map(str::to_owned).collect();
        let rows: &[Row] = if self.empty { &[] } else { ROWS };
        let items: Vec<Value> = rows
            .iter()
            .filter(|row| !self.hidden.contains(row.0))
            .filter(|row| match source.as_deref() {
                None | Some("all") => true,
                Some("you") => row.2.is_none(),
                Some(bot) => row.2 == Some(bot),
            })
            .filter(|row| kind.as_deref().is_none_or(|kind| kind_of(row.1) == kind))
            .filter(|row| {
                let bot = row.2.and_then(|id| BOTS.iter().find(|bot| bot.0 == id)).map_or("you", |bot| bot.1);
                let said = format!("{}\n{bot}\n{}", row.1, if row.3 { "MacBook Pro" } else { "Mac Studio" }).to_lowercase();
                words.iter().all(|word| said.contains(word))
            })
            .map(|row| item(row, now))
            .collect();
        let mut computers = vec![json!({ "key": "key_here", "name": "MacBook Pro", "here": true, "state": "here" })];
        if !self.empty {
            computers.push(json!({ "key": "key_studio", "name": "Mac Studio", "state": "there" }));
        }
        json!({
            "items": items, "more": false,
            "sources": { "bots": if self.empty { json!([]) } else { json!(BOTS.iter().map(|bot| json!({ "id": bot.0, "name": bot.1, "look": { "color": bot.2, "shape": bot.3, "face": "calm" }, "computer": "key_here" })).collect::<Vec<_>>()) }, "you": !self.empty },
            "computers": computers,
        })
    }

    /// What it answers of `kuvo/library…` (`query`: the request's), as JSON or bytes.
    pub fn answer(&mut self, method: &str, route: &str, query: &dyn Fn(&str) -> Option<String>, now: f64) -> Option<(u16, &'static str, Vec<u8>)> {
        let tail = route.strip_prefix("kuvo/library")?;
        let json = |status: u16, value: Value| Some((status, "application/json", value.to_string().into_bytes()));
        if tail.is_empty() && method == "GET" {
            return json(200, self.list(query, now));
        }
        let mut parts = tail.trim_start_matches('/').splitn(2, '/');
        let id = parts.next()?;
        let row = ROWS.iter().find(|row| row.0 == id && !self.empty)?;
        match (method, parts.next()) {
            ("GET", None) => json(200, item(row, now)),
            ("GET", Some("thumbnail")) => thumbnail(row).map(|bytes| (200, "image/png", bytes)).or_else(|| json(404, json!({ "message": "There's no thumbnail of it." }))),
            ("GET", Some("file")) => Some((200, "application/octet-stream", thumbnail(row).unwrap_or_else(|| b"sample".to_vec()))),
            ("POST", Some("hide")) => {
                self.hidden.insert(id.to_owned());
                let mut value = item(row, now);
                value["hidden"] = json!(now / 1000.);
                json(200, value)
            }
            _ => None,
        }
    }

    /// Each one following it hears it again, as its query lists it.
    pub fn tell(&self, now: f64) {
        for (path, sender) in self.followers.values() {
            let query = |name: &str| super::query(path, name).map(super::decoded);
            let _ = sender.send(self.list(&query, now).to_string());
        }
    }

    pub fn now_line(&self, path: &str, now: f64) -> String {
        let query = |name: &str| super::query(path, name).map(super::decoded);
        self.list(&query, now).to_string()
    }
}

// ---- Thumbnails, drawn as a box would send them (PNG) ----

type Color = [f32; 3];

fn hsl(h: f32, s: f32, l: f32) -> Color {
    let h = h.rem_euclid(360.) / 360.;
    let q = if l < 0.5 { l * (1. + s) } else { l + s - l * s };
    let p = 2. * l - q;
    let channel = |mut t: f32| {
        t = t.rem_euclid(1.);
        if t < 1. / 6. { p + (q - p) * 6. * t } else if t < 0.5 { q } else if t < 2. / 3. { p + (q - p) * (2. / 3. - t) * 6. } else { p }
    };
    [channel(h + 1. / 3.), channel(h), channel(h - 1. / 3.)]
}

struct Canvas {
    image: RgbaImage,
}

impl Canvas {
    fn new(w: u32, h: u32, color: Color) -> Self {
        let mut canvas = Canvas { image: RgbaImage::new(w, h) };
        canvas.rect(0., 0., 1., 1., color, 1.);
        canvas
    }
    fn w(&self) -> f32 {
        self.image.width() as f32
    }
    fn h(&self) -> f32 {
        self.image.height() as f32
    }
    fn blend(&mut self, x: u32, y: u32, color: Color, alpha: f32) {
        if x >= self.image.width() || y >= self.image.height() {
            return;
        }
        let pixel = self.image.get_pixel_mut(x, y);
        for (at, value) in color.iter().enumerate() {
            pixel.0[at] = ((pixel.0[at] as f32) * (1. - alpha) + value * 255. * alpha).round().clamp(0., 255.) as u8;
        }
        pixel.0[3] = 255;
    }
    /// A rectangle, in fractions of the picture, with rounded corners (`r`, in pixels).
    fn round(&mut self, fx: f32, fy: f32, fw: f32, fh: f32, r: f32, color: Color, alpha: f32) {
        let (x0, y0, x1, y1) = (fx * self.w(), fy * self.h(), (fx + fw) * self.w(), (fy + fh) * self.h());
        for y in y0.max(0.) as u32..y1.min(self.h()).ceil() as u32 {
            for x in x0.max(0.) as u32..x1.min(self.w()).ceil() as u32 {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let dx = (x0 + r - px).max(px - (x1 - r)).max(0.);
                let dy = (y0 + r - py).max(py - (y1 - r)).max(0.);
                if dx * dx + dy * dy <= r * r {
                    self.blend(x, y, color, alpha);
                }
            }
        }
    }
    fn rect(&mut self, fx: f32, fy: f32, fw: f32, fh: f32, color: Color, alpha: f32) {
        self.round(fx, fy, fw, fh, 0., color, alpha);
    }
    fn circle(&mut self, fx: f32, fy: f32, fr: f32, color: Color, alpha: f32) {
        let (cx, cy, r) = (fx * self.w(), fy * self.h(), fr * self.h());
        for y in (cy - r).max(0.) as u32..(cy + r).min(self.h()) as u32 {
            for x in (cx - r).max(0.) as u32..(cx + r).min(self.w()) as u32 {
                let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
                if dx * dx + dy * dy <= r * r {
                    self.blend(x, y, color, alpha);
                }
            }
        }
    }
    /// Top to bottom, from `top` to `bottom`.
    fn gradient(&mut self, top: Color, bottom: Color) {
        let h = self.image.height();
        for y in 0..h {
            let t = y as f32 / h as f32;
            let color = [top[0] + (bottom[0] - top[0]) * t, top[1] + (bottom[1] - top[1]) * t, top[2] + (bottom[2] - top[2]) * t];
            for x in 0..self.image.width() {
                self.blend(x, y, color, 1.);
            }
        }
    }
    /// Everything under the curve `y(x)` (fractions), filled.
    fn below(&mut self, curve: impl Fn(f32) -> f32, color: Color, alpha: f32) {
        for x in 0..self.image.width() {
            let top = (curve(x as f32 / self.w()) * self.h()).max(0.) as u32;
            for y in top..self.image.height() {
                self.blend(x, y, color, alpha);
            }
        }
    }
    /// The curve `y(x)` drawn as a line `width` pixels wide.
    fn line(&mut self, curve: impl Fn(f32) -> f32, from: f32, to: f32, width: f32, color: Color) {
        let steps = (self.w() * (to - from) * 2.) as u32;
        for at in 0..=steps {
            let fx = from + (to - from) * at as f32 / steps as f32;
            self.circle(fx, curve(fx), width / self.h() / 2., color, 1.);
        }
    }
    fn png(self) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(self.image).write_to(&mut out, image::ImageFormat::Png).ok();
        out.into_inner()
    }
}

fn thumbnail(row: &Row) -> Option<Vec<u8>> {
    let (.., motif, hue) = *row;
    let white = [1., 1., 1.];
    let ink = [0.16, 0.16, 0.17];
    let picture = |color: Color| Canvas::new(480, 304, color);
    let canvas = match motif {
        Motif::None => return None,
        Motif::Sunset => {
            let mut c = picture(white);
            c.gradient(hsl(hue + 200., 0.45, 0.42), hsl(hue, 0.55, 0.78));
            c.circle(0.62, 0.42, 0.16, hsl(hue + 28., 0.8, 0.9), 0.95);
            c.below(|x| 0.62 - 0.08 * (x * 6.).sin() * (x * 2.).cos(), hsl(hue + 235., 0.28, 0.32), 0.9);
            c.rect(0., 0.74, 1., 0.26, hsl(hue + 225., 0.35, 0.16), 0.95);
            c.round(0.48, 0.8, 0.28, 0.025, 3., hsl(hue + 22., 0.6, 0.86), 0.5);
            c
        }
        Motif::Botanical => {
            let mut c = picture(hsl(hue + 40., 0.28, 0.9));
            c.gradient(hsl(hue + 40., 0.28, 0.92), hsl(hue + 22., 0.22, 0.84));
            for (cx, cy, rx, ry, angle, light) in [(0.42, 0.5, 0.14, 0.42, -0.5f32, 0.42), (0.6, 0.55, 0.13, 0.38, 0.45f32, 0.55)] {
                let (w, h) = (c.w(), c.h());
                let color = hsl(hue, 0.3, light);
                for y in 0..c.image.height() {
                    for x in 0..c.image.width() {
                        let (dx, dy) = ((x as f32 / w - cx) * w / h, y as f32 / h - cy);
                        let (u, v) = (dx * angle.cos() + dy * angle.sin(), -dx * angle.sin() + dy * angle.cos());
                        if (u / rx).powi(2) + (v / ry).powi(2) <= 1. {
                            c.blend(x, y, color, 0.85);
                        }
                    }
                }
            }
            c.circle(0.28, 0.98, 0.22, hsl(20., 0.45, 0.72), 0.95);
            c
        }
        Motif::UiLight | Motif::UiDark => {
            let dark = motif == Motif::UiDark;
            let mut c = picture(if dark { hsl(hue, 0.25, 0.1) } else { hsl(hue, 0.35, 0.88) });
            let panel = if dark { hsl(hue, 0.12, 0.15) } else { white };
            let line = if dark { hsl(hue, 0.1, 0.24) } else { hsl(hue, 0.1, 0.88) };
            c.round(0.08, 0.1, 0.84, 0.8, 8., panel, 1.);
            c.round(0.1, 0.13, 0.2, 0.74, 6., if dark { hsl(hue, 0.12, 0.12) } else { hsl(hue, 0.15, 0.96) }, 1.);
            c.round(0.13, 0.2, 0.12, 0.04, 3., hsl(hue, 0.5, if dark { 0.45 } else { 0.7 }), 1.);
            for at in 0..3 {
                c.round(0.13, 0.3 + at as f32 * 0.09, 0.1, 0.03, 3., line, 1.);
            }
            c.round(0.36, 0.18, 0.3, 0.05, 3., line, 1.);
            c.round(0.36, 0.32, 0.22, 0.22, 6., line, 1.);
            c.round(0.62, 0.32, 0.25, 0.22, 6., hsl(hue, 0.45, if dark { 0.25 } else { 0.85 }), 1.);
            c.round(0.36, 0.62, 0.5, 0.04, 3., line, 1.);
            c.round(0.36, 0.71, 0.36, 0.04, 3., line, 1.);
            c
        }
        Motif::ChartLine => {
            let mut c = picture(hsl(hue, 0.15, 0.97));
            for at in 0..4 {
                c.rect(0.1, 0.25 + at as f32 * 0.17, 0.8, 0.006, hsl(hue, 0.1, 0.86), 1.);
            }
            let curve = |x: f32| 0.7 - 0.12 * ((x - 0.1) * 9.).sin().max(-0.4) - 0.35 * ((x - 0.1) / 0.8).powi(3);
            c.below(|x| if (0.1..0.9).contains(&x) { curve(x) } else { 2. }, hsl(hue, 0.45, 0.75), 0.45);
            c.line(curve, 0.1, 0.9, 4., hsl(hue, 0.7, 0.35));
            c
        }
        Motif::ChartBars => {
            let mut c = picture(hsl(hue, 0.12, 0.12));
            for (at, height) in [0.26f32, 0.42, 0.34, 0.56, 0.48, 0.38, 0.7].into_iter().enumerate() {
                c.round(0.16 + at as f32 * 0.1, 0.84 - height, 0.06, height, 3., hsl(hue, 0.35, 0.3 + height * 0.35), 1.);
            }
            c
        }
        Motif::Diagram => {
            let mut c = picture(hsl(hue, 0.1, 0.96));
            let card = white;
            c.round(0.12, 0.22, 0.2, 0.18, 8., card, 1.);
            c.round(0.12, 0.6, 0.2, 0.18, 8., card, 1.);
            c.round(0.7, 0.4, 0.18, 0.18, 8., card, 1.);
            c.rect(0.32, 0.31, 0.12, 0.012, hsl(hue, 0.1, 0.75), 1.);
            c.rect(0.32, 0.69, 0.12, 0.012, hsl(hue, 0.1, 0.75), 1.);
            c.round(0.42, 0.38, 0.2, 0.24, 8., hsl(hue, 0.45, 0.82), 1.);
            c.round(0.46, 0.48, 0.12, 0.035, 3., hsl(hue, 0.55, 0.45), 1.);
            c
        }
        Motif::Warm | Motif::Cool => {
            let mut c = picture(white);
            c.gradient(hsl(hue, 0.75, 0.86), hsl(hue + 20., 0.65, 0.66));
            c.circle(0.48, 0.48, 0.5, hsl(hue + 4., 0.55, 0.62), 0.6);
            c.circle(0.72, 0.68, 0.42, hsl(hue + 30., 0.75, 0.82), 0.7);
            c
        }
        Motif::Phone => {
            let mut c = picture(white);
            c.gradient(hsl(hue, 0.45, 0.88), hsl(hue + 20., 0.4, 0.76));
            c.round(0.36, 0.08, 0.28, 0.86, 18., white, 1.);
            c.round(0.46, 0.12, 0.08, 0.02, 3., hsl(hue, 0.05, 0.86), 1.);
            c.round(0.39, 0.22, 0.22, 0.2, 8., hsl(hue, 0.5, 0.86), 1.);
            c.round(0.39, 0.48, 0.22, 0.1, 6., hsl(hue, 0.05, 0.9), 1.);
            c.round(0.39, 0.63, 0.22, 0.1, 6., hsl(hue, 0.05, 0.9), 1.);
            c
        }
        Motif::Icons => {
            let mut c = picture(hsl(hue, 0.1, 0.97));
            for (at, shift) in [0f32, 40., 110., 190., 230., 300.].into_iter().enumerate() {
                let (x, y) = (0.22 + (at % 3) as f32 * 0.21, 0.2 + (at / 3) as f32 * 0.36);
                c.round(x, y, 0.15, 0.24, 12., hsl(hue + shift, 0.5, 0.72), 1.);
            }
            c
        }
        Motif::Studio => {
            let mut c = picture(white);
            c.gradient(hsl(hue, 0.25, 0.8), hsl(hue, 0.3, 0.55));
            c.rect(0., 0.72, 1., 0.28, hsl(hue, 0.25, 0.4), 1.);
            c.round(0.3, 0.2, 0.4, 0.38, 6., hsl(hue, 0.05, 0.16), 1.);
            c.round(0.32, 0.23, 0.36, 0.32, 3., hsl(200., 0.3, 0.6), 1.);
            c.rect(0.47, 0.58, 0.06, 0.12, hsl(hue, 0.05, 0.75), 1.);
            c
        }
        Motif::Pdf => {
            // A page, as Quick Look makes one: the sheet alone.
            let mut c = Canvas::new(232, 300, white);
            c.round(0.12, 0.08, 0.5, 0.05, 2., ink, 0.75);
            let accent = hsl(hue, 0.55, 0.45);
            if (hue as u32) % 3 == 0 {
                for at in 0..9 {
                    c.round(0.12, 0.2 + at as f32 * 0.07, if at % 3 == 2 { 0.5 } else { 0.76 }, 0.022, 1., ink, 0.16);
                }
            } else {
                c.round(0.12, 0.2, 0.7, 0.022, 1., ink, 0.16);
                c.round(0.12, 0.26, 0.6, 0.022, 1., ink, 0.16);
                c.round(0.12, 0.34, 0.76, 0.26, 3., accent, 0.16);
                for (at, height) in [0.08f32, 0.13, 0.18].into_iter().enumerate() {
                    c.rect(0.2 + at as f32 * 0.16, 0.56 - height, 0.1, height, accent, 0.8);
                }
                c.round(0.12, 0.66, 0.7, 0.022, 1., ink, 0.16);
                c.round(0.12, 0.72, 0.56, 0.022, 1., ink, 0.16);
            }
            c
        }
        Motif::Page => {
            // A page it built, as Quick Look shows it.
            let mut c = Canvas::new(480, 304, white);
            let accent = hsl(hue, 0.55, 0.45);
            c.rect(0., 0., 1., 0.12, hsl(hue, 0.15, 0.96), 1.);
            c.round(0.36, 0.04, 0.28, 0.04, 3., hsl(hue, 0.1, 0.86), 1.);
            c.round(0.26, 0.24, 0.48, 0.07, 4., accent, 0.9);
            c.round(0.32, 0.36, 0.36, 0.04, 3., hsl(hue, 0.1, 0.86), 1.);
            c.round(0.4, 0.46, 0.2, 0.08, 8., accent, 1.);
            for at in 0..3 {
                c.round(0.12 + at as f32 * 0.27, 0.64, 0.22, 0.22, 6., hsl(hue, 0.25, 0.94), 1.);
            }
            c
        }
    };
    Some(canvas.png())
}
