#!/usr/bin/python3
"""Authored semantic break inputs; independent browser measurements only."""
import json
from pathlib import Path

cases = []


def add(name, body, css='', width=100):
    cases.append(dict(name=name, body=f'<div id=p>{body}</div>', css=css, width=width, height=800))


for mode in ['normal', 'nowrap', 'pre', 'pre-wrap', 'pre-line', 'break-spaces']:
    for body in ['A<br>B', 'A <br> B', '<br>', '<br><br>', 'A<br>', 'A<wbr>B',
                 '<wbr>', 'A <wbr> B', 'A<br>\nB', 'A\n<br>B']:
        add(mode + '-' + body, body, f'#p{{white-space:{mode}}}', 30)
for width in [20, 40, 70, 100]:
    add('word-opportunity-' + str(width), 'AB<wbr>CD', width=width)
    add('adjacent-opportunities-' + str(width), 'A<wbr><wbr>B', width=width)
for body in ['A<br>B', 'A<wbr>B']:
    add('rtl-' + body, body, '#p{direction:rtl}', 70)
    add('inline-slice-' + body, '<span id=s>' + body + '</span>', '#s{margin-left:5px;padding-left:7px;padding-right:11px;margin-right:3px}', 70)
    add('after-inline-' + body, '<span id=s>' + body + '</span>C', '#s{margin-left:5px;padding-left:7px;padding-right:11px;margin-right:3px}', 70)
for body in ['A<br></span>', 'A<br><br></span>', 'A<br> </span>']:
    add('trailing-slice-' + body, '<span id=s>' + body, '#s{padding-left:7px;padding-right:11px}', 70)
add('hidden-br', 'A<br style="display:none">B')
add('hidden-wbr', 'A<wbr style="display:none">B', width=30)
add('visibility-br', 'A<br style="visibility:hidden">B')
add('display-contents-br', 'A<br style="display:contents">B')
add('display-block-br', 'A<br style="display:block">B')
add('display-contents-wbr', 'A<wbr style="display:contents">B', width=30)
Path(__file__).with_name('inputs.json').write_text(json.dumps(cases, indent=2) + '\n')
print(len(cases), 'inputs')
