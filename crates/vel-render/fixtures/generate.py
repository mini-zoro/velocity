from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.feaLib.builder import addOpenTypeFeaturesFromString
from pathlib import Path
p=Path('crates/vel-render/fixtures');p.mkdir(exist_ok=True)
fb=FontBuilder(1000,isTTF=True)
names=['.notdef','space','hyphen','greater','exclam','equal','x','arrow','notequal','altx']
fb.setupGlyphOrder(names)
fb.setupCharacterMap({32:'space',45:'hyphen',62:'greater',33:'exclam',61:'equal',120:'x',**{ord(c):'x' for c in 'ab❤☺✔⚠©↔☀♥✌#*0123456789क्ष'}})
glyphs={}
for i,name in enumerate(names):
 pen=TTGlyphPen(None)
 if name!='space':
  pen.moveTo((50,0));pen.lineTo((50,600+i*10));pen.lineTo((500,600+i*10));pen.lineTo((500,0));pen.closePath()
 glyphs[name]=pen.glyph()
fb.setupGlyf(glyphs)
fb.setupHorizontalMetrics({name:(600,0) for name in names})
fb.setupHorizontalHeader(ascent=800,descent=-200,lineGap=100)
fb.setupNameTable({'familyName':'Velocity Fixture','styleName':'Regular','uniqueFontIdentifier':'VelocityFixture-Regular-1','fullName':'Velocity Fixture Regular','psName':'VelocityFixture-Regular','version':'Version 1.0'})
fb.setupOS2(sTypoAscender=800,sTypoDescender=-200,sTypoLineGap=100,usWinAscent=800,usWinDescent=200)
fb.setupPost();fb.setupMaxp()
addOpenTypeFeaturesFromString(fb.font,'languagesystem DFLT dflt; feature liga {sub hyphen greater by arrow;} liga; feature calt {sub exclam equal by notequal;} calt; feature ss01 {sub x by altx;} ss01;')
fb.font['head'].created=fb.font['head'].modified=3800000000
fb.save(p/'ligature-fixture.ttf')

# A second original fixture covers each requested script and private-use character.
for table in fb.font['cmap'].tables:
    if table.isUnicode():
        table.cmap.update({ord(c):'x' for c in '한ไअאع⠿\ue000⛑◌\u0301⌚⌛⏰☕⚡\u20e3'})
        table.cmap.update({ord(c):'space' for c in '\ufe0e\ufe0f\u200d'})
# Supplementary math alphabet needs a format-12 Unicode cmap.
from fontTools.ttLib.tables._c_m_a_p import CmapSubtable
cmap=CmapSubtable.newSubtable(12)
cmap.platformID=3;cmap.platEncID=10;cmap.language=0
cmap.cmap={**fb.font['cmap'].tables[0].cmap,**{ord(c):'x' for c in '𝒜'}}
fb.font['cmap'].tables.append(cmap)
for name in fb.font['name'].names:
    if name.nameID==6: name.string='VelocityFallback-Regular'.encode(name.getEncoding())
fb.save(p/'fallback-fixture.ttf')

# Presentation fixture: the base font already covers the BMP text forms.
# A separate face lets mocked tests prove emoji takes priority over that cmap.
for table in fb.font['cmap'].tables:
    if table.isUnicode():
        table.cmap.update({ord(c):'x' for c in '❤☺✔⚠©↔☀♥✌#*0123456789क्ष⌚⌛⏰☕⚡'})
        # Original blank control glyphs keep synthetic sequences free of .notdef;
        # this fixture tests cascade/cluster coverage, not native emoji composition.
        table.cmap.update({ord(c):'space' for c in '\ufe0e\ufe0f\u200d\u20e3'})
        if table.format==12:
            table.cmap.update({ord(c):'x' for c in '👩💻🇨🇳👍🏽😀🌡🖥'})
for name in fb.font['name'].names:
    if name.nameID==6: name.string='VelocityEmoji-Regular'.encode(name.getEncoding())
from fontTools.colorLib.builder import buildCOLR, buildCPAL
fb.font['COLR']=buildCOLR({'x':[('x',0)]},glyphMap=fb.font.getReverseGlyphMap())
fb.font['CPAL']=buildCPAL([[(1.0,0.0,0.0,1.0)]])
fb.save(p/'emoji-fixture.ttf')
