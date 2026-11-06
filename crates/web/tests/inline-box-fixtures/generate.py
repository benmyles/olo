#!/usr/bin/python3
"""Authored inline-box spacing cases; no engine output used as expectations."""
import json
from pathlib import Path

cases = []


def add(name, body, css='', width=200):
    cases.append(dict(name=name, body=f'<div id=p>{body}</div>', css=css, width=width, height=800))


for width in [40, 70, 110, 200]:
    for edge in ['margin-left:10px;margin-right:5px', 'padding-left:7px;padding-right:11px',
                 'margin-left:-15px;margin-right:5px', 'margin-left:auto;margin-right:auto',
                 'padding-left:5%;padding-right:calc(5% + .3px)',
                 'margin-left:-7px;padding-left:7px']:
        add(f'wrap-{width}-{edge}', 'A <span id=s>B C D</span> E', '#s{' + edge + '}', width)
add('nested', 'A<span id=s>B<span id=t>C</span>D</span>E', '#s{margin-left:3px;padding-right:7px}#t{padding-left:11px;margin-right:-5px}')
add('block-interruption', 'A<span id=s>B<div id=b>C</div>D</span>E', '#s{margin-left:10px;margin-right:5px}')
add('empty-before-block', '<span id=s><div id=b>C</div></span>', '#s{padding-left:10px;padding-right:5px}')
add('empty', '<span id=s></span>', '#s{margin-left:10px;padding-right:5px}')
add('empty-middle', 'A<span id=s></span>B', '#s{padding-left:10px;padding-right:5px}')
add('empty-end', 'A<span id=s></span>', '#s{margin-left:10px;margin-right:5px}')
add('empty-after-space', 'A <span id=s></span>B', '#s{padding-left:10px;padding-right:5px}', 40)
add('nested-empty', 'A<span id=s><span id=t></span></span>B', '#s{padding-left:5px;padding-right:7px}#t{margin-left:11px;margin-right:13px}')
add('vertical-margins-ignored', 'A<span id=s>B</span>C', '#s{margin-top:400px;margin-bottom:200px}')
add('vertical-padding', 'A<span id=s>B</span>C', '#s{padding-top:30px;padding-bottom:40px}')
add('vertical-padding-empty', '<span id=s></span>', '#s{padding-top:30px;padding-bottom:40px}')
add('leading-space', '<span id=s> B C </span>', '#s{padding-left:10px;padding-right:5px}', 50)
add('collapsed-span', 'A <span id=s> </span> B', '#s{margin-left:10px;margin-right:5px}')
add('only-spaces', '<span id=s>   </span>', '#s{padding-left:10px;padding-right:5px}')
add('zero-empty', '<span id=s></span>', '#s{margin-left:auto;margin-right:auto}')
add('negative-return', 'A B<span id=s> C</span> D', '#s{margin-left:-60px;margin-right:10px}', 30)
for scale_length in ['.1px', '.3px', '10.123px', '33.333%', 'calc(5% + .2px)']:
    add('fraction-' + scale_length, 'A<span id=s>B C</span>D', '#s{padding-left:' + scale_length + ';margin-right:' + scale_length + '}', 101.25)
Path(__file__).with_name('inputs.json').write_text(json.dumps(cases, indent=2) + '\n')
print(len(cases), 'inputs')
