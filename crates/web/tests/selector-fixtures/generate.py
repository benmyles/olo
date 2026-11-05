#!/usr/bin/python3
"""Extract literal expectations and static XML trees; never run a CSS engine.
Run from this directory with /usr/bin/python3 generate.py. Sources remain
unmodified. XML fixtures test the generic tree interface, not XML loading.
"""
import ast
import json
from pathlib import Path
from xml.etree import ElementTree as ET
ROOT = Path(__file__).parent

def qname(value):
    if value.startswith('{'):
        namespace, name = value[1:].split('}', 1)
        return namespace, name
    return '', value

def document(file):
    parser = ET.XMLParser(target=ET.TreeBuilder(insert_comments=True, insert_pis=True))
    root = ET.parse(ROOT / file, parser=parser).getroot()
    if file == 'content.xhtml':
        # Exactly the upstream get_test_document namespace-test setup.
        parent = root.find(".//*[@id='root']")
        for ident in ('any-namespace', 'no-namespace'):
            div = ET.SubElement(parent, '{http://www.w3.org/1999/xhtml}div', id=ident)
            for n, ns in enumerate(['http://www.w3.org/1999/xhtml', 'http://www.w3.org/1999/xhtml', '', 'http://www.example.org/ns'], 1):
                ET.SubElement(div, '{%s}div' % ns if ns else 'div', id=f'{ident}-div{n}')
    # Shakespeare tests search an inclusive body subtree, as upstream does.
    if file == 'shakespeare.html':
        root = root.find('.//{http://www.w3.org/1999/xhtml}body')
    nodes = []
    def add(kind, parent, **data):
        index = len(nodes)
        nodes.append(dict(kind=kind, parent=parent, first=None, previous=None, next=None, **data))
        return index
    def visit(element, parent):
        if not isinstance(element.tag, str):
            return add('other', parent)
        ns, name = qname(element.tag)
        index = add('element', parent, ns=ns, name=name, attrs=[dict(ns=qname(k)[0], name=qname(k)[1], value=v) for k,v in element.attrib.items()])
        children = []
        if element.text: children.append(add('text', index, value=element.text))
        for child in element:
            children.append(visit(child, index))
            if child.tail: children.append(add('text', index, value=child.tail))
        if children: nodes[index]['first'] = children[0]
        for prev, nxt in zip(children, children[1:]):
            nodes[prev]['next'] = nxt
            nodes[nxt]['previous'] = prev
        return index
    visit(root, None)
    return nodes

documents = {name: document(name) for name in ('ids.html','content.xhtml','shakespeare.html')}
all_ids = [next((a['value'] for a in n.get('attrs',[]) if a['name']=='id' and not a['ns']), 'nil') for n in documents['ids.html'] if n['kind']=='element']
def literal(node):
    if isinstance(node, ast.Name) and node.id == 'ALL_IDS': return all_ids
    if isinstance(node, (ast.Tuple,ast.List)): return [literal(n) for n in node.elts]
    return ast.literal_eval(node)
cases=[]
for fn in ast.parse((ROOT/'test_cssselect2.py').read_text()).body:
    if not isinstance(fn,ast.FunctionDef) or fn.name not in ('test_select','test_html_select','test_select_shakespeare'): continue
    values=fn.decorator_list[0].args[1].elts
    for index, node in enumerate(values,1):
        xfail=isinstance(node,ast.Call)
        selector,expected=[literal(arg) for arg in node.args[:2]] if xfail else literal(node)
        cases.append(dict(suite=fn.name,case=index,selector=selector,expected=expected,upstream_xfail=xfail,document='shakespeare.html' if fn.name.endswith('shakespeare') else 'ids.html', html=fn.name=='test_html_select',mode='count' if fn.name.endswith('shakespeare') else 'ids'))
for assignment in ast.parse((ROOT/'w3_selectors.py').read_text()).body:
    if not isinstance(assignment,ast.Assign): continue
    suite=assignment.targets[0].id
    for index,test in enumerate(ast.literal_eval(assignment.value),1):
        cases.append(dict(suite=suite,case=index,document='content.xhtml',html=False,mode='invalid' if suite.startswith('invalid') else 'ids',selector=test['selector'],expected=test.get('expect'),exclude=test.get('exclude',[]),name=test['name']))
(ROOT/'derived.json').write_text(json.dumps(dict(documents=documents,cases=cases),ensure_ascii=False,indent=2)+'\n')
