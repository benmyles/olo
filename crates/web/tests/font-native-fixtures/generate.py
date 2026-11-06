#!/usr/bin/python3
"""Original small fonts from literal geometry/metadata, using fontTools 4.60.1.

No Olo engine, matcher, generated engine output or browser expectations are
consulted. Generated numeric references use fontTools' independent instancer.
Run using an isolated interpreter under /var/tmp, never install into the session.
"""
from pathlib import Path
import hashlib
import json
import fontTools
from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.ttLib import TTFont, TTCollection
from fontTools.designspaceLib import DesignSpaceDocument, AxisDescriptor, SourceDescriptor
from fontTools.varLib import build
from fontTools.varLib.instancer import instantiateVariableFont
from fontTools.varLib.varStore import VarStoreInstancer
from fontTools.varLib.models import normalizeLocation
from fontTools.ttLib import newTable
from fontTools.ttLib.tables._f_v_a_r import Axis

assert fontTools.__version__ == '4.60.1'
ROOT = Path(__file__).resolve().parent
OUT = ROOT/'fonts'
OUT.mkdir(exist_ok=True)
STAMP = 3406620150

def make(name, family, weight=400, width=5, style='normal', upem=1000,
         zero=600, water=None, vertical=False, metrics=True, aliases=(),
         xheight=500, capheight=700, ascent=800, descent=-200, gap=100):
    glyphs = ['.notdef', 'x', 'H']
    cmap = {ord('x'): 'x', ord('H'): 'H'}
    if zero is not None:
        glyphs += ['space','zero'];cmap.update({32:'space',48:'zero'})
    if water is not None:
        glyphs += ['water'];cmap[ord('水')]='water'
    fb = FontBuilder(upem, isTTF=True)
    fb.setupGlyphOrder(glyphs)
    fb.setupCharacterMap(cmap)
    outlines = {}
    advances = {}
    for glyph in glyphs:
        pen = TTGlyphPen(None)
        height = xheight if glyph == 'x' else capheight
        if glyph != 'space':
            pen.moveTo((0,0));pen.lineTo((300,0));pen.lineTo((300,height));pen.lineTo((0,height));pen.closePath()
        outlines[glyph] = pen.glyph()
        advance = zero if glyph == 'zero' else water if glyph == 'water' else upem//2
        advances[glyph] = (advance, 0)
    fb.setupGlyf(outlines)
    fb.setupHorizontalMetrics(advances)
    fb.setupHorizontalHeader(ascent=ascent,descent=descent,lineGap=gap)
    subfamily = style.title() if style != 'normal' else 'Regular'
    fb.setupNameTable(dict(familyName=family,styleName=subfamily,uniqueFontIdentifier=name,
                          fullName=family+' '+subfamily,psName='Olo-'+name))
    flags = (1 if style == 'italic' else 512 if style == 'oblique' else 64 if weight<700 else 0) | (32 if weight>=700 else 0) | (128 if metrics else 0)
    fb.setupOS2(sTypoAscender=ascent,sTypoDescender=descent,sTypoLineGap=gap,
                usWinAscent=ascent,usWinDescent=-descent,usWeightClass=weight,
                usWidthClass=width,sxHeight=xheight,sCapHeight=capheight,fsSelection=flags)
    fb.font['OS/2'].version = 4 if metrics else 0
    fb.setupPost(italicAngle=-12 if style=='italic' else -20 if style=='oblique' else 0)
    fb.setupMaxp()
    fb.font['head'].macStyle=(1 if weight>=700 else 0)|(2 if style=='italic' else 0)
    fb.font['head'].created=STAMP;fb.font['head'].modified=STAMP
    for alias, language in aliases:
        fb.font['name'].setName(alias,16,3,1,language)
    if vertical:
        fb.setupVerticalMetrics({g:(2300 if g=='zero' else 2400,0) for g in glyphs})
        fb.setupVerticalHeader(ascent=ascent,descent=descent,lineGap=gap)
    fb.font.recalcTimestamp=False
    fb.save(OUT/(name+'.ttf'))

for name,weight,width,style in [
    ('regular',400,5,'normal'),('light',300,5,'normal'),('medium',500,5,'normal'),
    ('bold',700,5,'normal'),('narrow',400,3,'normal'),('narrow-italic',900,3,'italic'),
    ('expanded',700,7,'normal'),('italic',400,5,'italic'),('oblique',400,5,'oblique')]:
    make(name,'Olo Match',weight,width,style)
make('metrics-first','Olo Metrics',zero=None)
make('metrics-fallback','Olo Fallback',upem=2000,zero=1300,water=2200,vertical=True)
make('metrics-missing','Olo Missing',zero=None,metrics=False)
make('metrics-composite','Olo Composite',zero=None,metrics=False)
composite=TTFont(OUT/'metrics-composite.ttf',recalcTimestamp=False)
pen=TTGlyphPen(composite.getGlyphSet());pen.addComponent('H',(1,0,0,0.5,0,0));composite['glyf']['x']=pen.glyph();composite.save(OUT/'metrics-composite.ttf')
make('unicode','Straße',aliases=[('書体',0x0411),('Σ',0x0408)])
make('composed','å')
make('decomposed','a\u030a')

collection=TTCollection();collection.fonts=[TTFont(OUT/'regular.ttf',recalcTimestamp=False),TTFont(OUT/'metrics-fallback.ttf',recalcTimestamp=False)]
collection.save(OUT/'collection.ttc',shareTables=True)

# Compatible masters with distinct advances AND MVAR-sensitive metrics.
doc=DesignSpaceDocument();axis=AxisDescriptor();axis.name='Optical size';axis.tag='opsz';axis.minimum=8;axis.default=16;axis.maximum=72;doc.addAxis(axis)
for size,zero,xheight,capheight,ascent in [(8,500,480,680,780),(16,600,500,700,800),(72,1200,600,800,900)]:
    name='optical-master-'+str(size)
    make(name,'Olo Optical',zero=zero,xheight=xheight,capheight=capheight,ascent=ascent)
    src=SourceDescriptor();src.path=str(OUT/(name+'.ttf'));src.name=name;src.location={'Optical size':size}
    if size==16:src.copyInfo=True;src.copyLib=True;src.copyFeatures=True
    doc.addSource(src)
font,_,_=build(doc);font.recalcTimestamp=False;font['head'].created=STAMP;font['head'].modified=STAMP;font.save(OUT/'optical.ttf')
for p in OUT.glob('optical-master-*.ttf'):p.unlink()
phantom=TTFont(OUT/'optical.ttf',recalcTimestamp=False);del phantom['HVAR'];phantom.save(OUT/'optical-phantom.ttf')

# Original metadata-only variable face: axes need not change glyph outlines.
# Deliberately extend wght beyond CSS's 1..1000 interval to exercise clamping.
make('axes','Olo Axes')
variable=TTFont(OUT/'axes.ttf',recalcTimestamp=False);fvar=newTable('fvar');fvar.axes=[];fvar.instances=[]
for i,(tag,minimum,default,maximum) in enumerate([('wght',0,400,1200),('wdth',50,100,200),('slnt',-30,0,20),('ital',-1,0,1)]):
    axis=Axis();axis.axisTag=tag;axis.minValue=minimum;axis.defaultValue=default;axis.maxValue=maximum;axis.flags=0;axis.axisNameID=256+i;fvar.axes.append(axis)
    variable['name'].setName(tag,axis.axisNameID,3,1,0x409)
variable['fvar']=fvar;variable.save(OUT/'axes.ttf')

references=[]
for css_size,opsz in [(16,16),(20,20),(32,32),(72,72),(120,72),(20,12)]:
    inst=instantiateVariableFont(TTFont(OUT/'optical.ttf',recalcTimestamp=False),{'opsz':opsz},inplace=False)
    os2=inst['OS/2'];upem=inst['head'].unitsPerEm
    references.append(dict(css_size=css_size,opsz=opsz,zero_units=inst['hmtx'].metrics['zero'][0],x_units=os2.sxHeight,cap_units=os2.sCapHeight,ascent_units=os2.sTypoAscender,descent_units=os2.sTypoDescender,line_gap_units=os2.sTypoLineGap,upem=upem))
(ROOT/'optical-references.json').write_text(json.dumps(dict(fonttools=fontTools.__version__,scope='Independent fontTools instancer readings. Integer table readings may be rounded; raw HVAR/MVAR delta comparisons are recorded separately.',cases=references),indent=2)+'\n')

# Raw variation-table evaluation, independently of Olo/ttf-parser. Quantize the
# normalized input to the signed 2.14 coordinate accepted by the Rust parser.
def raw_metrics(path, css_size, coordinates):
    font=TTFont(path,recalcTimestamp=False)
    axes=font['fvar'].axes if 'fvar' in font else []
    ranges={a.axisTag:(a.minValue,a.defaultValue,a.maxValue) for a in axes}
    location=normalizeLocation(coordinates,ranges)
    if 'avar' in font:
        from fontTools.varLib.models import piecewiseLinearMap
        location={tag:piecewiseLinearMap(value,font['avar'].segments[tag]) for tag,value in location.items()}
    location={tag:int(value*16384)/16384 for tag,value in location.items()}
    zero=font.getBestCmap()[48];adv=font['hmtx'].metrics[zero][0]
    if 'HVAR' in font:
        table=font['HVAR'].table
        index=table.AdvWidthMap.mapping[zero] if table.AdvWidthMap else font.getGlyphID(zero)
        adv+=VarStoreInstancer(table.VarStore,axes,location)[index]
    os2=font['OS/2'];metrics=dict(ch=adv,ex=os2.sxHeight,cap=os2.sCapHeight,ascent=os2.sTypoAscender,descent=os2.sTypoDescender,line_gap=os2.sTypoLineGap)
    if 'MVAR' in font:
        table=font['MVAR'].table;inst=VarStoreInstancer(table.VarStore,axes,location)
        tags=dict(xhgt='ex',cpht='cap',hasc='ascent',hdsc='descent',hlgp='line_gap')
        for record in table.ValueRecord:
            if record.ValueTag in tags:metrics[tags[record.ValueTag]]+=inst[record.VarIdx]
    return dict(css_size=css_size,coordinates=coordinates,normalized=location,units=metrics,upem=font['head'].unitsPerEm,sha256=hashlib.sha256(path.read_bytes()).hexdigest())

raw=[raw_metrics(OUT/'optical.ttf',size,{'opsz':min(max(size,8),72)}) for size in [8,12,16,20,32,72,120]]
noto=ROOT.parents[3]/'data/fonts/NotoSans[wght].ttf'
real=[raw_metrics(noto,20,{'wght':weight}) for weight in [400,650.5,900]]
(ROOT/'raw-references.json').write_text(json.dumps(dict(fonttools=fontTools.__version__,scope='Independent raw HVAR/MVAR reads at normalized 2.14 coordinates; not shaping or rasterization.',optical=raw,noto=real),indent=2)+'\n')
print('Generated',len(list(OUT.iterdir())),'fonts; optical references',len(references))
