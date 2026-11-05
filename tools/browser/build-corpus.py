#!/usr/bin/python3
"""Rebuild the curated 100-site manifest against a pinned ranking snapshot.

Usage: /usr/bin/python3 tools/browser/build-corpus.py /var/tmp/.../tranco-JZPGY.csv
The source CSV is downloaded separately; no network is used here.
"""
import hashlib
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[2]
SHA256 = "218ab1bd9fb2074559928bdef831c33539651ce5904eff3b78f31798f5b59de7"
GROUPS = {
    "search": "google.com bing.com yahoo.com duckduckgo.com baidu.com yandex.ru",
    "community": "facebook.com instagram.com x.com reddit.com tiktok.com linkedin.com pinterest.com threads.net snapchat.com quora.com tumblr.com weibo.com zhihu.com discord.com mastodon.social",
    "media": "youtube.com netflix.com twitch.tv spotify.com soundcloud.com vimeo.com dailymotion.com bilibili.com disneyplus.com hulu.com primevideo.com kick.com",
    "content": "wikipedia.org stackoverflow.com stackexchange.com fandom.com imdb.com medium.com wordpress.com blogger.com nytimes.com bbc.com cnn.com theguardian.com reuters.com washingtonpost.com coursera.org khanacademy.org",
    "commerce": "amazon.com ebay.com walmart.com etsy.com aliexpress.com taobao.com jd.com shopify.com target.com bestbuy.com ikea.com temu.com mercadolibre.com",
    "maps-travel": "booking.com airbnb.com tripadvisor.com expedia.com uber.com lyft.com openstreetmap.org flightaware.com kayak.com",
    "apps": "microsoft.com apple.com github.com gitlab.com notion.so figma.com canva.com miro.com trello.com asana.com slack.com zoom.us office.com dropbox.com box.com proton.me fastmail.com linear.app replit.com codesandbox.io",
    "platforms": "adobe.com salesforce.com hubspot.com atlassian.com vercel.com cloudflare.com paypal.com stripe.com openai.com",
}
JOURNEYS = {
    "search": "Enter the fixed query 'Olo browser engine', inspect results, open one result, scroll, return through history and verify query/results restoration.",
    "community": "Open a fixed public profile or topic, use its public search/filter, inspect a thread and its expanded content, scroll and return through history. Login walls require a dedicated eval account; otherwise record blocked. Do not post, react, follow or send messages.",
    "media": "Find a fixed playable public test item, inspect the media page, play/pause/seek/mute, scroll and return through history. Subscription/authentication requires a dedicated eval account; otherwise record blocked.",
    "content": "Find a fixed reference article or lesson, open its full content, expand a section or follow an in-page anchor, scroll and return through history. Paywalls require a dedicated eval subscription; otherwise record blocked.",
    "commerce": "Search a fixed product, filter results, open a detail page and switch a photo or variant, scroll and return through history. Never add to cart, purchase, book or change an account.",
    "maps-travel": "Search a fixed destination or location, inspect results and a detail/map view, change a local filter or map zoom, scroll and return through history. Never book, request transport or change an account.",
    "apps": "Open the site's actual application in a dedicated eval workspace, inspect a fixed document/project, use search/navigation, type and undo an unsent local draft, scroll and return through history. Marketing pages do not satisfy this journey. Record blocked without the required test workspace/account.",
    "platforms": "Open an interactive public demo or dedicated eval dashboard, inspect a fixed item, change a read-only view/filter, scroll and return through history. Marketing pages do not satisfy this journey. Never submit payments, publish, send messages or change an account; record blocked when no test dashboard is available.",
}
FEATURES = {
    "search": ("search_results", "Query was entered and matching result content appeared; input and result activation both worked."),
    "community": ("thread_interaction", "Public topic/profile opened and expanding/filtering changed its visible content."),
    "media": ("media_controls", "Playback, pause, seek and mute affected decoded media, with sound isolated from the person's session."),
    "content": ("article_navigation", "Full article/lesson content and an in-page anchor or expanded section worked."),
    "commerce": ("product_interaction", "Search, filtering and product photo/variant interaction changed the correct content."),
    "maps-travel": ("location_interaction", "Destination/location search and a map/view/filter interaction changed the correct content."),
    "apps": ("application_interaction", "The real app opened its test document/project, accepted input and undo without submitting or publishing."),
    "platforms": ("dashboard_interaction", "A real interactive demo/dashboard opened and its read-only filtering changed visible content."),
}


def main():
    data = Path(sys.argv[1]).read_bytes()
    assert hashlib.sha256(data).hexdigest() == SHA256, "Wrong ranking bytes; use pinned list JZPGY"
    ranks = {domain: int(rank) for rank, domain in (line.split(',') for line in data.decode().splitlines())}
    common = [
        ("document_ready", "Navigation committed and finished the recorded checkpoint without a renderer crash or security/network error."),
        ("meaningful_content", "Expected site content is visible, not a blank, bot challenge, consent wall, error, login wall or marketing substitute."),
        ("scroll_changed", "User scrolling changed the visible content and agent geometry consistently."),
        ("navigation_changed", "A real user/agent activation navigated to the intended content with the expected URL and history entry."),
        ("history_restored", "Back returned to the previous document, state and scroll position."),
    ]
    sites = []
    for category, domains in GROUPS.items():
        for domain in domains.split():
            sites.append({"id": domain.replace('.', '-'), "origin": f"https://{domain}/", "domain": domain, "rank": ranks[domain], "category": category, "journey": JOURNEYS[category], "checks": [{"id": key, "description": value} for key, value in common + [FEATURES[category]]]})
    assert len(sites) == len({s['domain'] for s in sites}) == 100
    suite = {
        "schema_version": 1,
        "selection": {
            "kind": "curated-popularity-and-complexity-coverage",
            "description": "100 distinct ranked domains selected for popular-site and complex-application coverage across eight categories. This is NOT the numeric top 100, a measured complexity ranking, or an unbiased compatibility sample. Each full application journey needs independently captured, site-specific checkpoints; category recipes are requirements, not recorded successes.",
            "ranking_url": "https://tranco-list.eu/download/JZPGY/1000000",
            "ranking_sha256": SHA256,
            "ranking_date": "2026-11-04",
        },
        "matrix": {"viewports": [{"id": "desktop", "width": 1280, "height": 800}, {"id": "narrow", "width": 390, "height": 844}], "scales": [100, 125, 150, 175, 200], "themes": ["light", "dark", "night"]},
        "sites": sites,
    }
    out = ROOT / "docs/browser/eval"
    (out / "sites.json").write_text(json.dumps(suite, indent=2, ensure_ascii=False) + '\n')
    (out / "selected-ranks.csv").write_text('rank,domain\n' + ''.join(f"{s['rank']},{s['domain']}\n" for s in sorted(sites, key=lambda s: s['rank'])))
    print(f"Wrote {len(sites)} sites, 3000 configurations, 12000 required checkpoints; no website runs claimed")


if __name__ == '__main__':
    main()
