"""Console background only: every panel's foreground (headings, labels, knobs, buttons, meters,
EQ curve) hidden with visibility:hidden so each panel keeps its true size; gradient washes and
the page-wide grain stay exactly as the browser renders them. 2x pixel density."""
from playwright.sync_api import sync_playwright
import sys
SRC, OUT = sys.argv[1], sys.argv[2]
with sync_playwright() as p:
    b = p.chromium.launch()
    pg = b.new_page(viewport={"width": 1280, "height": 800}, device_scale_factor=2)
    pg.goto("file://" + SRC); pg.wait_for_timeout(800)
    pg.add_style_tag(content=".mast>*:not(.bloom),.cb>*:not(.fill),.p808>*:not(.sky){visibility:hidden!important}body::after{content:none!important}")
    pg.wait_for_timeout(200)
    box = pg.locator("#app").bounding_box()
    pg.screenshot(path=OUT, clip=box)
    print(box)
    b.close()
