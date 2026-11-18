#!/usr/bin/env python3
"""What Leaflet's own leaflet_formats.py makes of a set of files, for the
Rust port to be checked against (formats/tests/oracle.rs).

    python3 tools/formats_oracle.py LEAFLET_DIR OUT_DIR

Builds a corpus in OUT_DIR/corpus (Leaflet's test fixtures, the extra
documents below, and documents written from a set of editor HTML snippets),
then writes OUT_DIR/oracle.json and, for every document, what the Python
saves: OUT_DIR/out/<name>.<how>.<format>.
"""
import json
import os
import subprocess
import sys

leaflet_dir, out_dir = sys.argv[1], sys.argv[2]
sys.path.insert(0, leaflet_dir)
import leaflet_formats as lf  # noqa: E402

corpus = os.path.join(out_dir, 'corpus')
saved = os.path.join(out_dir, 'out')
os.makedirs(corpus, exist_ok=True)
os.makedirs(saved, exist_ok=True)

# Leaflet's own fixtures (no personal data; made from scratch).
subprocess.run([sys.executable, os.path.join(leaflet_dir, 'tests', 'fixtures.py'), corpus], check=True,
               stdout=subprocess.DEVNULL)

TINY_PNG = ('iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAFklEQVR4nGP8z8DAwMDAxMDAwMDAAAANHQEDasKb6QAAAABJRU5ErkJggg==')

HTML = {
    'plain': '<p>Hello world</p>',
    'empty': '',
    'emptypara': '<p><br></p>',
    'inline': '<p>A <b>bold</b>, <i>italic</i>, <u>under</u>, <s>strike</s>, x<sup>2</sup> and H<sub>2</sub>O.</p>',
    'nested': '<p><b><i>both</i> just bold</b> <span style="font-weight: bold; font-style: italic">styled</span></p>',
    'spaces': '<p>  lead   and&nbsp;&nbsp;&nbsp;gaps\tand\ttabs  </p>',
    'headings': ''.join(f'<h{n}>Heading {n}</h{n}>' for n in range(1, 7)),
    'align': '<p style="text-align: center">mid</p><p style="text-align: right">right</p><p style="text-align: justify">just</p><div align="center">div</div>',
    'links': '<p><a href="https://example.test/a?b=1&amp;c=2">web</a> <a href="mailto:a@example.test">mail</a> <a href="javascript:alert(1)">bad</a> <a href="file:///etc/passwd">file</a></p>',
    'lists': '<ul><li>one</li><li>two<ul><li>two.a</li><li>two.b<ol><li>deep</li></ol></li></ul></li><li><p>para item</p><p>second para</p></li></ul><ol><li>first</li><li style="text-align: center">centred</li></ol>',
    'listinlist': '<ul><ul><li>orphan</li></ul><li>after</li></ul>',
    'table': '<table><tbody><tr><th>H1</th><th>H2</th></tr><tr><td colspan="2">wide</td></tr><tr><td><p>a</p><p>b</p></td><td></td></tr></tbody></table>',
    'pagebreak': '<p>before</p><hr class="page-break"><p>after</p><hr><p>end</p>',
    'breaks': '<p>line one<br>line two<br></p><br><br><p>x</p>',
    'image': f'<p>pic <img src="data:image/png;base64,{TINY_PNG}" width="40" height="30" alt="a &quot;tiny&quot; one"> after</p>'
             f'<p><img src="data:image/png;base64,{TINY_PNG}" style="width: 700px; height: 100px"></p>'
             f'<p><img src="https://example.test/x.png"></p>',
    'drop': '<p>keep<script>alert(1)</script><style>p{}</style><svg><text>no</text></svg> this</p><button>no</button>',
    'unknown': '<section><blockquote><p>quoted</p></blockquote><article>free text <em>em</em></article></section><custom-tag>custom</custom-tag>',
    'mixed': 'loose text <b>bold</b><p>para</p>tail text',
    'entities': '<p>&lt;tag&gt; &amp; &quot;q&quot; &#233; &#x263A; &copy; &mdash; caf&eacute;</p>',
    'unicode': '<p>Ünïcödé 日本語 עברית 😀 \u0001control</p>',
    'deco': '<p><span style="text-decoration: underline line-through">both</span> <span style="vertical-align: super">up</span> <ins>ins</ins> <del>del</del></p>',
    'comment': '<p>a<!-- hidden -->b</p>',
    'divs': '<div>one</div><div></div><div><br></div><div><p>inner</p>text</div>',
    'pre': '<pre>  code   here\n  more</pre><address>addr</address>',
}

for name, html in HTML.items():
    doc = lf.from_html(html)
    for fmt in ('docx', 'odt'):
        with open(os.path.join(corpus, f'html-{name}.{fmt}'), 'wb') as f:
            f.write(lf.save(doc, fmt))

EXTRA = {
    'rtf-basic.rtf': rb'{\rtf1\ansi\ansicpg1252\deff0{\fonttbl{\f0 Arial;}}{\info{\author Someone}}'
                     rb'\pard\qc{\b Bold} and {\i italic}\par\pard Caf\'e9 \u8364? euro \u-10179?\u-8704? smile\line next\par'
                     rb'\trowd\cellx1000\cellx2000\intbl a\cell b\cell\row\pard after table\par'
                     rb'{\header head}{\footnote note}\page \ul under\ulnone  \super sup\nosupersub  done\par}',
    'rtf-cp1251.rtf': rb'{\rtf1\ansi\ansicpg1251 \'cf\'f0\'e8\'e2\'e5\'f2\par}',
    'rtf-uc.rtf': rb'{\rtf1\uc2 \u12354\'82\'a0 x\uc0\u12356 y\par}',
    'rtf-empty.rtf': rb'{\rtf1}',
    'txt-utf8.txt': 'one\ntwo\r\nthree\n\x0c\nfour  spaced\ttab\n'.encode('utf-8'),
    'txt-bom.txt': '\ufeffwith bom\n'.encode('utf-8'),
    'txt-utf16.txt': 'sixteen\nbits\n'.encode('utf-16'),
    'txt-cp1252.txt': 'caf\xe9 \x80 \x81'.encode('latin-1'),
    'txt-empty.txt': b'',
    'ole.doc': b'\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1' + b'\0' * 100,
    'binary.bin': b'\x00\x01\x02garbage',
}
for name, data in EXTRA.items():
    with open(os.path.join(corpus, name), 'wb') as f:
        f.write(data)


def zip_parts(data):
    import io
    import zipfile
    with zipfile.ZipFile(io.BytesIO(data)) as z:
        return [[i.filename, i.compress_type] for i in z.infolist()]


results = {}
for name in sorted(os.listdir(corpus)):
    with open(os.path.join(corpus, name), 'rb') as f:
        data = f.read()
    r = {'kind': lf.detect(data, name)}
    if name.endswith(('.jpg', '.png', '.gif')) or lf.sniff_image(data):
        r['strip'] = name + '.strip'
        with open(os.path.join(saved, r['strip']), 'wb') as f:
            f.write(lf.strip_image_metadata(data))
        r['size'] = list(lf.image_size(data))
    if r['kind'] in ('docx', 'odt', 'rtf', 'txt'):
        try:
            doc = lf.load(data, r['kind'])
        except lf.FormatError as e:
            r['error'] = str(e)
        else:
            r['html'] = lf.to_html(doc)
            r['warnings'] = doc.warnings
            back = lf.from_html(r['html'])
            r['rehtml'] = lf.to_html(back)
            r['plain'] = lf.plain_text(doc.blocks)
            r['outputs'] = {}
            for how, d in (('direct', doc), ('edited', back)):
                for fmt in ('docx', 'odt', 'txt'):
                    try:
                        out = lf.save(d, fmt)
                    except UnicodeEncodeError:
                        # Python keeps RTF's \\u surrogate pairs as two halves; the page
                        # joins them, so the app only ever saves the 'edited' path.
                        continue
                    fname = f'{name}.{how}.{fmt}'
                    with open(os.path.join(saved, fname), 'wb') as f:
                        f.write(out)
                    r['outputs'][fname] = zip_parts(out) if fmt != 'txt' else None
    results[name] = r

for name, html in HTML.items():
    back = lf.from_html(html)
    results['html:' + name] = {'source': html, 'rehtml': lf.to_html(back), 'plain': lf.plain_text(back.blocks)}

with open(os.path.join(out_dir, 'oracle.json'), 'w', encoding='utf-8') as f:
    json.dump(results, f, ensure_ascii=True, indent=1)
print(f'oracle: {len(results)} cases in {out_dir}')
