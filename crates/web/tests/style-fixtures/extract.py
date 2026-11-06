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
for path in sorted((ROOT/'wpt').rglob('*.html')):
 text=path.read_text();d=Data(text);file=str(path.relative_to(ROOT/'wpt'));styles=Page(text).styles
 unported.append(dict(file=file,reason='JavaScript/CSSOM support, serialization and round-trip driver'))
 def location(i):return dict(file=file,line=text.count('\n',0,d.positions[i])+1,offset=d.positions[i])
 for i,t in enumerate(d.tokens[:-1]):
  if t not in ['test_computed_value','test_display_affected','test_relative','test_relative_size','test_font_size'] or d.tokens[i+1]!='(' or i and d.tokens[i-1]=='function':continue
  end=d.end[i+1];args=[];at=i+2
  while at<end:
   value,after=d.value(at)
   if value is UNKNOWN or after<end and d.tokens[after]!=',':args=None;break
   args.append(value);at=after+1
  if args is None:
   unported.append(dict(**location(i),reason='dynamic/nonliteral expected-value driver'));continue
  if t=='test_computed_value':
   property,specified=args[:2];expected=args[2] if len(args)>2 else specified
   cases.append(dict(**location(i),kind=t,property=property,specified=specified,expected=expected,styles=styles))
  elif t=='test_relative':
   specified,inherited,expected=args
   cases.append(dict(**location(i),kind=t,property='font-weight',specified=specified,expected=expected,parent_inline='font-weight:'+inherited,styles=styles))
  elif t=='test_display_affected':
   property,value=args;displays=None
   for j,token in enumerate(d.tokens[:-2]):
    if token=='displayValues' and d.tokens[j+1]=='=':displays=d.value(j+2)[0];break
   assert displays is not None
   for specified,expected in [('inline-table','table')]+[(v,'block') for v in displays]+[('inline-flex','flex'),('inline-grid','grid'),('list-item','list-item'),('contents','contents')]:
    cases.append(dict(**location(i),kind=t,property='display',specified=specified,expected=expected,extra_inline=property+':'+value,styles=styles))
  else:unported.append(dict(**location(i),reason=t+' requires UA font policy/presentational hints and browser driver'))
 if file.endswith('border-width-computed.html'):
  unported.append(dict(file=file,reason='dynamic CSSOM thin/medium/thick ordering driver'))

scope='Original HTML/stylesheet/selector/cascade/computed-style document jobs against pinned literal WPT assertions. Native semantic values and selected resolved values, no CSSOM serializer or JavaScript. Missing container/other stylesheet contexts stay unsupported; no website, rendering, font selection or layout evidence.'
(ROOT/'derived.json').write_text(json.dumps(dict(scope=scope,cases=cases,unported=unported),indent=2)+'\n')
print(len(cases),'native assertions;',len(unported),'unported browser drivers')
