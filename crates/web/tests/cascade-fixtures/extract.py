#!/usr/bin/python3
"""Literal data ports only. No JS or color library execution/golden generation."""
from pathlib import Path
import ast,json,re
ROOT=Path(__file__).parent
UNKNOWN=object()
SKIP={'skip': True}
TOKEN=re.compile(r'''\s+|//[^\n]*|/\*[\s\S]*?\*/|"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|`(?:\\.|[^`\\])*`|[A-Za-z_$][\w$]*|(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?|.''')
class Data:
 def __init__(self,text):
  self.text=text;self.tokens=[];self.positions=[]
  for m in TOKEN.finditer(text):
   token=m[0]
   if token.isspace() or token.startswith(('//','/*')):continue
   self.tokens.append(token);self.positions.append(m.start())
  self.end={};self.parent={};stack=[]
  for i,t in enumerate(self.tokens):
   if t in ('{','[','('):
    if stack:self.parent[i]=stack[-1]
    stack.append(i)
   elif t in ('}',']',')') and stack:
    start=stack.pop();self.end[start]=i
 def value(self,i,env=None):
  t=self.tokens[i]
  if t.startswith(('"',"'")):return ast.literal_eval(t),i+1
  if t.startswith('`'):
   s=t[1:-1]
   names=re.findall(r'\$\{(\w+)\}',s)
   if names and (not env or any(n not in env for n in names)):return UNKNOWN,i+1
   return re.sub(r'\$\{(\w+)\}',lambda m:env[m[1]],s),i+1
  if t in ('null','undefined','NaN'):return (None if t!='undefined' else SKIP),i+1
  if t in ('true','false'):return t=='true',i+1
  if t=='-' and re.fullmatch(r'[\d.eE+-]+',self.tokens[i+1]):return -float(self.tokens[i+1]),i+2
  if re.fullmatch(r'(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?',t):return float(t),i+1
  if t=='[':
   result=[];i+=1
   while self.tokens[i]!=']':
    if self.tokens[i]==',':result.append(SKIP);i+=1;continue
    value,i=self.value(i,env)
    if value is UNKNOWN:return UNKNOWN,self.end.get(i-1,i)+1
    result.append(value)
    if self.tokens[i]==',':i+=1
    elif self.tokens[i]!=']':return UNKNOWN,i
   return result,i+1
  if t=='{':
   fields={}
   for key,index in self.fields(i).items():fields[key]=self.value(index,env)[0]
   return fields,self.end[i]+1
  if env and t in env:return env[t],i+1
  return UNKNOWN,self.end.get(i,i)+1
 def fields(self,start):
  fields={};i=start+1;end=self.end[start]
  while i<end:
   if i+1<end and self.tokens[i+1]==':':
    key=self.tokens[i];key=ast.literal_eval(key) if key.startswith(('"',"'")) else key
    fields[key]=i+2;i+=2
   if self.tokens[i] in ('{','[','('):i=self.end[i]+1
   else:i+=1
  return fields
 def ancestors(self,i):
  while i in self.parent:
   i=self.parent[i];yield i


from html.parser import HTMLParser
class Page(HTMLParser):
 def __init__(self,text):
  super().__init__(convert_charrefs=False);self.styles=[];self.style=None;self.feed(text)
 def handle_starttag(self,tag,attrs):
  if tag=='style':self.style=[]
 def handle_data(self,text):
  if self.style is not None:self.style.append(text)
 def handle_endtag(self,tag):
  if tag=='style' and self.style is not None:self.styles.append(''.join(self.style));self.style=None
cases=[];unported=[]
for path in sorted((ROOT/'wpt').glob('*.html')):
 text=path.read_text();d=Data(text);page=Page(text);found=False
 def location(i,helper):return {'file':path.name,'line':text.count('\n',0,d.positions[i])+1,'offset':d.positions[i],'helper':helper}
 # Port complete literal testCases arrays; upstream's assertion loops over
 # both target classes, or compares the inline target to its green reference.
 if path.name in ('layer-basic.html','layer-important.html','layer-vs-inline-style.html'):
  i=d.tokens.index('testCases');data,_=d.value(i+2)
  assert isinstance(data,list)
  for n,item in enumerate(data):
   if not isinstance(item,dict) or not isinstance(item.get('style'),str):
    unported.append(dict(location(i,'testCases'),case=n,reason='nonliteral style/JS array driver'));continue
   inline=path.name=='layer-vs-inline-style.html'
   assert 'rgb(0, 128, 0)' in text or inline and '#reference' in text and 'background-color: green' in text
   for target in (['#target'] if inline else ['target.first','target.second']):
    cases.append(dict(location(i,'testCases'),case=n,title=item.get('title'),target=target,property='background-color' if inline else 'color',styles=page.styles+[item['style']],inline=item.get('inlineStyle'),expected='rgb(0, 128, 0)',mode='color'))
  continue
 # Literal getComputedStyle(ID).property and .getPropertyValue('--name').
 pattern=r"assert_equals\(\s*getComputedStyle\(\s*([A-Za-z_$][\w$]*)\s*\)\.(?:([A-Za-z]+)|getPropertyValue\(\s*(['\"])(--[^'\"]+)\3\s*\))\s*(?:\.trim\(\))?\s*,\s*(['\"])([^'\"]*)\5"
 for m in re.finditer(pattern,text):
  target,camel,_,custom,_,expected=m.groups();prop=custom or re.sub(r'[A-Z]',lambda m:'-'+m[0].lower(),camel)
  cases.append({'file':path.name,'line':text.count('\n',0,m.start())+1,'offset':m.start(),'helper':'assert_equals/getComputedStyle','target':'#'+target,'property':prop,'styles':page.styles,'expected':expected,'mode':'custom' if custom else 'color' if prop in ('color','background-color') else 'numeric'})
  found=True
 # The alias-based computed-style case has independently literal references.
 if path.name=='layers-basic.html':
  for prop,expected in [('color','rgb(255, 255, 0)'),('background-color','rgb(0, 0, 0)'),('font-size','64px')]:cases.append({'file':path.name,'helper':'assert_equals/styles alias','target':'#testP','property':prop,'styles':page.styles,'expected':expected,'mode':'color' if prop!='font-size' else 'numeric'});found=True
 # These reftests' green background component is ported separately from
 # geometry/rasterization, using the unchanged reference's explicit green.
 if path.name.startswith('revert-layer-'):
  reference=(ROOT/'wpt/reference/ref-filled-green-100px-square.xht').read_text()
  assert 'background:green' in reference.replace(' ','') or 'background-color:green' in reference.replace(' ','')
  target='#inner' if path.name=='revert-layer-006.html' else '#target'
  cases.append({'file':path.name,'helper':'green-square reference/color component only','target':target,'property':'background-color','styles':page.styles,'expected':'rgb(0, 128, 0)','mode':'color','reference':'reference/ref-filled-green-100px-square.xht'})
  unported.append({'file':path.name,'helper':'reftest','reason':'geometry/rasterization reference driver not executed'});found=True
 if not found:unported.append({'file':path.name,'helper':'document','reason':'JS/document driver not ported'})
(ROOT/'derived.json').write_text(json.dumps({'scope':'Native stylesheet/selector/cascade/defaulting/custom-token and isolated color/numeric ports over inert HTML. Not JS/CSSOM/animation/layout/rendering/page execution; reftest colors exclude geometry/rasterization.','cases':cases,'unported':unported},ensure_ascii=False,indent=2)+'\n')
print(len(cases),'literal native cases;',len(unported),'unported drivers')
