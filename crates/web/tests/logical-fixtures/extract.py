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

def literal_variable(text,name):
 d=Data(text)
 for i,t in enumerate(d.tokens[:-2]):
  if t==name and d.tokens[i+1]=='=':return d.value(i+2)[0]
 raise ValueError(name)

cases=[];unported=[]
shared=(ROOT/'wpt/resources/test-shared.js').read_text()
modes=literal_variable(shared,'writingModes')
values=literal_variable((ROOT/'wpt/resources/test-box-properties.js').read_text(),'testValues')['length']
assert len(modes)==8 and sum(len(m['styles']) for m in modes)==10
def decl(fields):return ''.join(f'{p}:{v};' for p,v in fields.items())
def add(file,title,styles,expected,inline=None,html=None,target='#test'):
 for prop,value in expected:
  row=dict(file=file,title=title,styles=styles,target=target,mode='numeric',property=prop,expected=value)
  if inline is not None:row['inline']=inline
  if html is not None:row['html']=html
  cases.append(row)
def unsupported(file,title,reason):unported.append(dict(file=file,title=title,reason=reason))

# Port the actual helper's assertion loops using its literal mapping table
# and literal values. No engine output, browser result or generated golden is
# read here. The inert target is the DOM that the upstream JS helper creates.
helper_html='<!doctype html><div style="width:100px;height:100px"><div id="test" class="test"></div></div>'
for name in ['margin','padding','inset','size']:
 file=f'logical-box-{name}.html'
 groups=[]
 if name=='size':
  for prefix in ['', 'max-', 'min-']:
   groups.append((dict(inline=prefix+'inline-size',block=prefix+'block-size'),dict(horizontal=prefix+'width',vertical=prefix+'height'),{},'display:block;'))
 else:
  logical={axis+side.title():f'{name}-{axis}-{side}' for axis in ['inline','block'] for side in ['start','end']}
  physical={side:(side if name=='inset' else name+'-'+side) for side in ['left','right','top','bottom']}
  shorthands={name+'-'+axis:[logical[axis+'Start'],logical[axis+'End']] for axis in ['inline','block']}
  shorthands[name]=[physical[s] for s in ['top','right','bottom','left']]
  groups.append((logical,physical,shorthands,'position:relative;' if name=='inset' else ''))
 for logical,physical,shorthands,prerequisites in groups:
  logicals=list(logical.values());physicals=list(physical.values())
  unsupported(file,f'{logicals[0]}: style declaration support','CSSOM specified-value driver')
  for shorthand,longhands in shorthands.items():
   unsupported(file,f'{shorthand}: serialize specified value','CSSOM specified/shorthand serialization driver')
   add(file,'initial shorthand computed '+shorthand,['.test{'+prerequisites+shorthand+':'+ ' '.join(values[:len(longhands)])+'}'],list(zip(longhands,values)),html=helper_html)
  for mode in modes:
   for style in mode['styles']:
    wm=decl(style)
    associated={}
    for side,l in logical.items():
     p=physical[mode[side]];associated[l]=p;associated[p]=l
    rules=['.test{'+wm+'}', '.test{'+prerequisites+decl(dict(zip(logicals,values)))+'}']
    expected=[pair for p,v in zip(logicals,values) for pair in [(p,v),(associated[p],v)]]
    add(file,'shared aliases '+wm,rules,expected,html=helper_html)
    for shorthand,longhands in shorthands.items():
     expected=[pair for p,v in zip(longhands,values) for pair in [(p,v),(associated[p],v)]]
     add(file,'shorthand '+shorthand+' '+wm,['.test{'+wm+'}', '.test{'+prerequisites+shorthand+':'+ ' '.join(values[:len(longhands)])+'}'],expected,html=helper_html)
    for last_logical in [True,False]:
     lasts,others=(logicals,physicals) if last_logical else (physicals,logicals)
     for last in lasts:
      expected=[pair for i,p in enumerate(others) for pair in [(p,values[len(others)] if associated[p]==last else values[i]),(associated[p],values[len(others)] if associated[p]==last else values[i])]]
      add(file,'source order '+last+' '+wm,['.test{'+wm+prerequisites+decl(dict(zip(others,values)))+last+':'+values[len(others)]+'}'],expected,html=helper_html)
      # Upstream high-specificity rule intentionally precedes the lower one.
      add(file,'specificity '+last+' '+wm,['#test{'+last+':'+values[len(others)]+'} .test{'+wm+prerequisites+decl(dict(zip(others,values)))+'}'],expected,html=helper_html)

file='logicalprops-with-deferred-writing-mode.html'
text=(ROOT/'wpt'/file).read_text();styles=Page(text).styles
# Complete literal test bodies: cssText, expected object and title. Preserve
# ancestor stylesheet and class mutation; each native assertion is a fresh
# snapshot, not evidence of live CSSOM invalidation.
pattern=r'test\(function\(\)\s*\{([\s\S]*?)\},\s*"([^"]+)"\);'
for body,title in re.findall(pattern,text):
 inline=re.search(r'target.style.cssText\s*=\s*`([^`]+)`',body)[1]
 expected=Data(re.search(r'check\((\{[\s\S]*?\})\)',body)[1]).value(0)[0]
 html=text.replace('id="target"','id="target" class="revert-layer"') if 'target.className' in body else None
 add(file,title,styles,list(expected.items()),inline,html,target='#target')

file='logicalprops-with-variables-revert.html'
text=(ROOT/'wpt'/file).read_text();styles=Page(text).styles
for target,prop,value in re.findall(r'check\("([^"]+)", "([^"]+)", "([^"]+)"\)',text):
 add(file,'literal revert assertion',styles,[(prop,value)],target='#'+target)

file='logicalprops-with-variables.html'
text=(ROOT/'wpt'/file).read_text()
for body in re.findall(r'\{\s*title = ([\s\S]*?)\n\}',text):
 title=ast.literal_eval(body.split(';',1)[0].strip())
 inline=decl(dict(re.findall(r'style.setProperty\("([^"]+)", "([^"]+)"\)',body)))
 for prop,specified,computed in re.findall(r'check\("([^"]+)", "([^"]*)", "([^"]+)"\)',body):
  unsupported(file,title+' specified '+prop,'CSSOM specified-value driver')
  if prop=='margin-inline':unsupported(file,title+' computed '+prop,'CSSOM shorthand serialization driver')
  else:add(file,title,[],[(prop,computed)],inline,target='#target')

data=dict(scope='Pinned CSS logical native assertion ports. Literal WPT mapping/values and assertion loops; no JS, CSSOM, live mutation, layout, shaping, rasterization, UI or website evidence. CSSOM drivers remain unsupported.',cases=cases,unported=unported)
(ROOT/'derived.json').write_text(json.dumps(data,indent=2)+'\n')
print(len(cases),'native assertions;',len(unported),'unsupported drivers')
