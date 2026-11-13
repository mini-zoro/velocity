use anyhow::{bail,Result};
use bench_common::{arg,emit,stats,timed};
use cosmic_text::{fontdb,Attrs,Buffer,Family,FontSystem,Metrics,Shaping,SwashCache,Wrap};
use serde_json::json;
use std::{collections::HashSet,hint::black_box,path::Path};
fn viewport()->String {
 let mut lines=Vec::new();
 for n in 0..120 {
  let head=format!("    pub fn worker_{n:03}(input: &str) -> Result<usize, Error> {{ let value = map.get(\"key\"); // row {n:03} ");
  let mut line=head;while line.len()<200{line.push_str("let x = items[index] + 42; ");}line.truncate(200);assert!(line.is_ascii());lines.push(line);
 }lines.join("\n")
}
fn new_buffer(text:&str,shaping:Shaping)->Buffer {
 let mut b=Buffer::new_empty(Metrics::new(14.0,20.0));b.set_size(Some(2800.0),Some(2400.0));b.set_wrap(Wrap::None);
 b.set_text(text,&Attrs::new().family(Family::Name("Menlo")),shaping,None);b
}
fn glyphs(b:&Buffer)->usize{b.layout_runs().map(|r|r.glyphs.len()).sum()}
fn main()->Result<()> {
 let args=std::env::args().collect::<Vec<_>>();let font=arg(&args,"--font","/System/Library/Fonts/Menlo.ttc");
 if !Path::new(&font).exists(){bail!("font unavailable: {font}");}
 let (fs,load_ms)=timed(||->Result<FontSystem>{let mut db=fontdb::Database::new();db.load_font_file(&font)?;db.set_monospace_family("Menlo");Ok(FontSystem::new_with_locale_and_db("en-US".into(),db))});let mut fs=fs?;
 let faces=fs.db().faces().map(|f|json!({"families":f.families.iter().map(|(n,_)|n).collect::<Vec<_>>(),"post_script":f.post_script_name,"monospaced":f.monospaced})).collect::<Vec<_>>();let text=viewport();
 for (mode,shaping) in [("advanced",Shaping::Advanced),("basic",Shaping::Basic)] {
  let mut times=Vec::new();let mut first=0.0;let mut count=0;
  for i in 0..10 {
   let (b,ms)=timed(||{let mut b=new_buffer(&text,shaping);b.shape_until_scroll(&mut fs,false);black_box(glyphs(&b));b});
   assert_eq!(b.layout_runs().count(),120);count=glyphs(&b);assert_eq!(count,24000);
   if i==0{first=ms;}else{times.push(ms);}
  }
  let mut b=new_buffer(&text,shaping);b.shape_until_scroll(&mut fs,false);let mut edits=Vec::new();
  for i in 0..101 {
   let mut line=b.lines[60].text().to_owned();line.replace_range(199..200,if i%2==0{"A"}else{"B"});
   let (_,ms)=timed(||{let attrs=b.lines[60].attrs_list().clone();let ending=b.lines[60].ending();assert!(b.lines[60].set_text(&line,ending,attrs));b.shape_until_scroll(&mut fs,false);black_box(glyphs(&b));});
   if i>0 {edits.push(ms);}
  }
  let mut cached=Vec::new();for _ in 0..100 {let (_,ms)=timed(||{b.shape_until_scroll(&mut fs,false);black_box(glyphs(&b));});cached.push(ms);}
  let keys=b.layout_runs().flat_map(|r|r.glyphs.iter().map(|g|g.physical((0.0,r.line_y),1.0).cache_key).collect::<Vec<_>>()).collect::<Vec<_>>();let unique=keys.iter().collect::<HashSet<_>>().len();
  let mut swash=SwashCache::new();let (_,raster_cold_ms)=timed(||{for &key in &keys{black_box(swash.get_image(&mut fs,key));}});
  let mut raster_warm=Vec::new();for _ in 0..7 {let (_,ms)=timed(||{for &key in &keys{black_box(swash.get_image(&mut fs,key));}});raster_warm.push(ms);}
  let mut uncached=Vec::new();for _ in 0..5 {let (_,ms)=timed(||{for &key in &keys{black_box(swash.get_image_uncached(&mut fs,key));}});uncached.push(ms);}
  emit(json!({"bench":"text","mode":mode,"font":font,"font_faces":faces,"font_system_load_ms":load_ms,"viewport_lines":120,"columns":200,"glyphs":count,"first_full_layout_ms":first,"full_layout":stats(&times),"one_line_relayout":stats(&edits),"unchanged_cached_viewport":stats(&cached),"unique_raster_keys":unique,"raster_first_cached_pass_ms":raster_cold_ms,"raster_warm_cached_pass":stats(&raster_warm),"raster_without_cache":stats(&uncached),"note":"ASCII code, 14px Menlo, 20px line height, Wrap::None; full layout includes Buffer+set_text+shape+glyph count with warm FontSystem; edit is one changed line + viewport scan; headless, no GPU upload, atlas or frame time measured"}));
 }Ok(())
}
