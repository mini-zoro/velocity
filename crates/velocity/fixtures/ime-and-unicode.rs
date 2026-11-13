// Velocity shell fixture: UTF-8, tabs, CJK fallback, emoji, and marked text.
// Click after a Chinese character, type nihao with Pinyin, and choose 你好.
// No text in this file is compiled by the prototype.
fn main() {
    let greeting = "你好，世界！中文输入法候选窗口应跟随光标。";
    let kana = "かな カナ 日本語：変換と確定";
    let emoji = "🦀 😀 🚀 👩‍💻 ❤️ 🇨🇳";
    let accents = "é é naïve Ångström"; // NFC é and decomposed e + U+0301
    let full_width = "，。！？；：（）【】《》";
	let tabs = "tab before this line, and\tinside this string";
    let numbers = 0xff + 1_024;
    // Start composition here → 
    // Select part of this line with Shift+Arrow or mouse dragging.
    // Emoji sequences intentionally include ZWJ, VS16, and regional indicators.
    // Check their rendering separately from whether AppKit inserts them.
}

// Scroll the fixture while keeping a composition active, then resize the window.
// Candidate anchor should move to the visible caret, without a Retina 2× offset.
// Filler rows make PageUp/PageDown and fractional trackpad scrolling observable.
// 01 你好世界
// 02 你好世界
// 03 你好世界
// 04 你好世界
// 05 你好世界
// 06 你好世界
// 07 你好世界
// 08 你好世界
// 09 你好世界
// 10 你好世界
// 11 你好世界
// 12 你好世界
// 13 你好世界
// 14 你好世界
// 15 你好世界
// 16 你好世界
// 17 你好世界
// 18 你好世界
// 19 你好世界
// 20 你好世界
// 21 你好世界
// 22 你好世界
// 23 你好世界
// 24 你好世界
// 25 你好世界
// 26 你好世界
// 27 你好世界
// 28 你好世界
// 29 你好世界
// 30 你好世界
// 31 你好世界
// 32 你好世界
// 33 你好世界
// 34 你好世界
// 35 你好世界
// 36 你好世界
// 37 你好世界
// 38 你好世界
// 39 你好世界
// 40 你好世界
// 41 你好世界
// 42 你好世界
// 43 你好世界
// 44 你好世界
// 45 你好世界
// 46 你好世界
// 47 你好世界
// 48 你好世界
// 49 你好世界
// 50 你好世界
// End of fixture.
