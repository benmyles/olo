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

def dump(name,value): (ROOT/name).write_text(json.dumps(value,ensure_ascii=False,indent=2)+'\n')
# The original library has non-CSS extensions and JS-object color inputs.
# Select the standard color-space/literal CSS contract before any Olo run.
spaces={'srgb','srgb-linear','p3','p3-linear','display-p3','display-p3-linear','a98rgb','a98-rgb','prophoto','prophoto-rgb','rec2020','xyz','xyz-d65','xyz-d50','lab','lch','oklab','oklch','hsl','hwb'}
d=Data((ROOT/'colorjs-conversions.js').read_text());cases=[];excluded=[]
for start,t in enumerate(d.tokens):
 if t!='{' or start not in d.end:continue
 fields=d.fields(start)
 if 'args' not in fields or 'expect' not in fields:continue
 args=d.value(fields['args'])[0];expected=d.value(fields['expect'])[0];target=None;epsilon=0.0001
 for ancestor in list(reversed(list(d.ancestors(start))))+[start]:
  if d.tokens[ancestor]!='{':continue
  f=d.fields(ancestor)
  if 'data' in f:
   data=d.value(f['data'])[0]
   if isinstance(data,dict):
    if isinstance(data.get('toSpace'),str):target=data['toSpace']
    if isinstance(data.get('epsilon'),(int,float)):epsilon=data['epsilon']
 if isinstance(args,list) and len(args)==2:args,target=args
 line=d.text.count('\n',0,d.positions[start])+1
 reason=None
 if target not in spaces:reason='non-CSS/unsupported target or JS conversion hook'
 elif not isinstance(args,str):reason='non-literal/JS-object source'
 elif not isinstance(expected,list) or len(expected)!=3 or any(not (isinstance(x,(int,float)) or x is None or x==SKIP) for x in expected):reason='non-coordinate assertion'
 elif 'none' in args.lower():reason='missing-component propagation differs from explicit absolute conversion'
 elif re.match(r'color\(\s*--',args):reason='library custom-space aliases without CSS profile context'
 elif (m:=re.match(r'([\w-]+)\(',args)) and m[1] not in {'rgb','rgba','hsl','hsla','hwb','lab','lch','oklab','oklch','color'}:reason='non-CSS source function'
 elif (m:=re.match(r'color\(\s*([\w-]+)',args)) and m[1] not in {'srgb','srgb-linear','display-p3','display-p3-linear','a98-rgb','prophoto-rgb','rec2020','xyz','xyz-d50','xyz-d65'}:reason='source outside CSS Color 4 predefined spaces'
 if reason:excluded.append({'line':line,'reason':reason});continue
 cases.append({'line':line,'input':args,'target':target,'expected':expected,'epsilon':epsilon})
dump('derived-conversions.json',{'scope':'Literal standard-space absolute conversion vectors, not Color.js execution or missing-component propagation','cases':cases,'excluded':excluded})
# Port literal array rows/direct calls. Template loops in the predefined
# color() resource are expanded from their literal colorSpace lists only.
cases=[];unported=[]
for path in sorted((ROOT/'wpt').glob('*.html')):
 text=path.read_text();d=Data(text);mode='invalid' if 'invalid' in path.name else 'valid' if 'valid' in path.name else 'computed'
 for i,t in enumerate(d.tokens):
  if t=='tests' and d.tokens[i+1:i+3]==['=','[']:
   rows,_=d.value(i+2)
   if rows is UNKNOWN:raise ValueError(f'non-literal tests array: {path}')
   for n,row in enumerate(rows,1):
    if isinstance(row,str):row=[row]
    if not isinstance(row,list) or not isinstance(row[0],str):raise ValueError(row)
    cases.append({'file':path.name,'case':n,'mode':mode,'input':row[0],'expected':row[1] if mode=='computed' else None})
  if t not in ('test_computed_value','test_valid_value','test_invalid_value') or d.tokens[i+1]!='(':continue
  end=d.end[i+1];arg0,j=d.value(i+2)
  if arg0!='color' or d.tokens[j]!=',':continue
  input_index=j+1;input,j=d.value(input_index);expected=None
  if j<end and d.tokens[j]==',':expected,_=d.value(j+1)
  environments=[{}]
  if input is UNKNOWN and d.tokens[input_index].startswith('`'):
   environments=[]
   for ancestor in d.ancestors(i+1):
    if d.tokens[ancestor]!='{':continue
    # Only the known for (const colorSpace of [literal strings]) header.
    prefix=d.tokens[max(0,ancestor-40):ancestor]
    for h in range(len(prefix)-3):
     if prefix[h:h+3]==['const','colorSpace','of']:
      absolute=max(0,ancestor-40)+h+3
      labels,_=d.value(absolute)
      if isinstance(labels,list) and all(isinstance(x,str) for x in labels):environments=[{'colorSpace':s,'resultColorSpace':'xyz-d65' if s=='xyz' else s} for s in labels]
   if not environments:unported.append({'file':path.name,'offset':d.positions[i],'reason':'non-literal template environment'});continue
  for env in environments:
   value,j=d.value(input_index,env)
   expected=None
   if j<end and d.tokens[j]==',':expected,_=d.value(j+1,env)
   if not isinstance(value,str):
    # The tests-array loop is already ported above; do not count it twice.
    if d.tokens[input_index]!='test':unported.append({'file':path.name,'offset':d.positions[i],'reason':'non-literal direct call'})
    continue
   if mode=='computed' and not isinstance(expected,str):unported.append({'file':path.name,'input':value,'reason':'non-literal computed expectation'});continue
   cases.append({'file':path.name,'case':text.count('\n',0,d.positions[i])+1,'mode':mode,'input':value,'expected':expected})
dump('derived-wpt.json',{'scope':'Native ports of literal value assertions/template-space expansions; no WPT JS, CSSOM serialization, page/style/layout execution','cases':cases,'unported':unported})
print('Color.js vectors:',len(json.loads((ROOT/'derived-conversions.json').read_text())['cases']))
print('WPT assertions:',len(cases),'unported direct forms:',len(unported))
