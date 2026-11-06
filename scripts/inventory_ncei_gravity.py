#!/usr/bin/env python3
"""Inventory NOAA gravity map directories using HTML listings and HTTP HEAD only."""
import argparse
import concurrent.futures
import datetime
import hashlib
from html.parser import HTMLParser
import json
import pathlib
import re
import subprocess
import threading
import time
from urllib.parse import urljoin, urlsplit, urlunsplit
from ncei_gravity_maps import map_summary

CATALOG = 'https://www.ncei.noaa.gov/products/gravity-data'
DATA = 'https://www.ngdc.noaa.gov/mgg/gravity/1999/data/'
OCEAN_MAPS = 'https://www.ngdc.noaa.gov/mgg/gravity/2008/data/'
NUMERIC = {'.bin', '.ast', '.asc', '.dat', '.xyz', '.grd', '.nc', '.tif', '.img'}


class Tables(HTMLParser):
    """Retain table cells and link labels for both Drupal and Apache indexes."""
    def __init__(self, html):
        super().__init__(convert_charrefs=True)
        self.rows, self.row, self.cell, self.link = [], None, None, None
        self.feed(html)

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if tag == 'tr':
            self.row = []
        elif tag in {'td', 'th'} and self.row is not None:
            self.cell = {'text': '', 'links': []}
        elif tag == 'a' and self.cell is not None:
            self.link = {'href': attrs.get('href', ''), 'text': ''}

    def handle_data(self, text):
        if self.cell is not None:
            self.cell['text'] += text
        if self.link is not None:
            self.link['text'] += text

    def handle_endtag(self, tag):
        if tag == 'a' and self.link is not None:
            self.cell['links'].append(self.link)
            self.link = None
        elif tag in {'td', 'th'} and self.cell is not None:
            self.cell['text'] = ' '.join(self.cell['text'].split())
            self.row.append(self.cell)
            self.cell = None
        elif tag == 'tr' and self.row is not None:
            if self.row:
                self.rows.append(self.row)
            self.row = None


def normalize(url):
    p = urlsplit(url)
    host = p.hostname or ''
    if host in {'ngdc.noaa.gov', 'www.ngdc.noaa.gov'}:
        host = 'www.ngdc.noaa.gov'
    return urlunsplit(('https', host, p.path, '', ''))


def catalog_products(html):
    products = []
    for row in Tables(html).rows:
        links = [link for cell in row for link in cell['links'] if link['text'].strip() == 'Access']
        if links:
            products.append({'name': row[0]['text'], 'extent': row[1]['text'],
                             'url': normalize(urljoin(CATALOG, links[0]['href']))})
    return products


def listing(html, directory):
    entries = []
    for row in Tables(html).rows:
        if len(row) != 3 or not row[0]['links']:
            continue
        link = row[0]['links'][0]
        raw = urljoin(directory, link['href'])
        url = normalize(raw)
        # Parent/sort/navigation links must never escape the selected directory.
        if urlsplit(raw).query or not url.startswith(directory) or url == directory:
            continue
        entries.append({'url': url, 'name': link['text'].strip(),
                        'listed_size': row[2]['text'], 'last_modified_listed': row[1]['text']})
    return entries


def listed_bytes(value):
    m = re.fullmatch(r'(\d+(?:\.\d+)?)([KMGT]?)', value.strip())
    if not m:
        return None
    return round(float(m[1]) * 1024 ** (' KMGT'.index(m[2]) if m[2] else 0))


class Metadata:
    def __init__(self, root, interval=0.2):
        self.root, self.interval = root, interval
        root.mkdir(parents=True, exist_ok=True)
        self.lock, self.next_request = threading.Lock(), 0.0

    def fetch(self, url, head=False):
        key = hashlib.sha256((('HEAD ' if head else 'GET ') + url).encode()).hexdigest()
        path = self.root / (key + ('.head.json' if head else '.html'))
        if path.exists():
            return json.loads(path.read_text()) if head else path.read_text()
        with self.lock:
            delay = max(0, self.next_request - time.monotonic())
            self.next_request = max(time.monotonic(), self.next_request) + self.interval
        if delay:
            time.sleep(delay)
        cmd = ['curl', '-fLsS', '--connect-timeout', '10', '--max-time', '40',
               '--retry', '1', '-H', 'Accept-Encoding: identity',
               '-A', '1kEE-gravity-metadata-inventory/1.0']
        cmd += ['-I'] if head else ['--max-filesize', '2097152']
        result = subprocess.run(cmd + [url], capture_output=True, check=True)
        text = result.stdout.decode('utf-8', errors='replace')
        if head:
            blocks = [b for b in re.split(r'\r?\n\r?\n', text) if b.startswith('HTTP/')]
            if not blocks:
                raise ValueError('Missing HTTP response headers: ' + url)
            headers = dict((k.lower().strip(), v.strip()) for line in blocks[-1].splitlines()[1:]
                           if ':' in line for k, v in [line.split(':', 1)])
            size = headers.get('content-length')
            value = {'bytes': int(size) if size and size.isdigit() else None,
                     'last_modified': headers.get('last-modified'),
                     'content_type': headers.get('content-type')}
            path.write_text(json.dumps(value, indent=2) + '\n')
            return value
        path.write_text(text)
        return text


def numeric_key(url):
    p = pathlib.PurePosixPath(urlsplit(url).path)
    if p.suffix.lower() in {'.gz', '.z', '.zip'}:
        p = p.with_suffix('')
    return str(p.with_suffix('')) if p.suffix.lower() in NUMERIC else None


def crawl(client, roots):
    pending, seen, files, errors = set(roots), set(), {}, []
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        while pending:
            batch = sorted(pending - seen)
            pending.clear()
            if len(seen) + len(batch) > 500:
                raise RuntimeError('Directory limit exceeded; review scope')
            futures = [(url, pool.submit(client.fetch, url)) for url in batch]
            for url, future in futures:
                seen.add(url)
                try:
                    html = future.result()
                    if 'Index of ' not in html:
                        raise ValueError('Not a directory index')
                    for item in listing(html, url):
                        if item['url'].endswith('/'):
                            # lists/ contains alternate inventory documents; programs/ is software.
                            if item['name'] not in {'lists/', 'programs/'}:
                                pending.add(item['url'])
                        else:
                            files[item['url']] = item
                except Exception as e:
                    errors.append({'url': url, 'error': str(e)})
    return list(files.values()), errors, sorted(seen)


def measure(client, files):
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        futures = [(item, pool.submit(client.fetch, item['url'], True)) for item in files]
        for item, future in futures:
            try:
                item.update(future.result())
                item['size_source'] = 'HTTP Content-Length' if item['bytes'] is not None else 'unavailable'
            except Exception as e:
                item['bytes'], item['head_error'] = None, str(e)
                item['size_source'] = 'unavailable'
            item['listed_approx_bytes'] = listed_bytes(item['listed_size'])
            item['numeric_key'] = numeric_key(item['url'])


def describe(client, files):
    metadata = [item for item in files if pathlib.PurePosixPath(item['url']).suffix.lower()
                in {'.hdr', '.txt', '.doc', '.rdc', '.fmt', '.1st'} and item.get('bytes') is not None
                and item['bytes'] <= 65536]
    descriptions = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        futures = [(item, pool.submit(client.fetch, item['url'])) for item in metadata]
        for item, future in futures:
            try:
                descriptions.append({'url': item['url'], 'text': future.result()})
            except Exception as e:
                descriptions.append({'url': item['url'], 'error': str(e)})
    return descriptions


def summarize(files):
    groups = {}
    for item in files:
        if item['numeric_key']:
            groups.setdefault(item['numeric_key'], []).append(item)
    return {'file_count': len(files),
            'exact_sized_files': sum(item.get('bytes') is not None for item in files),
            'known_bytes_all_files': sum(item.get('bytes') or 0 for item in files),
            'numeric_file_count': sum(len(items) for items in groups.values()),
            'named_numeric_products': len(groups),
            'known_bytes_all_numeric_formats': sum(item.get('bytes') or 0 for item in files if item['numeric_key']),
            'smallest_representation_bytes': sum(min(item['bytes'] for item in items if item.get('bytes') is not None)
                                                  for items in groups.values() if any(item.get('bytes') is not None for item in items)),
            'numeric_products_missing_all_sizes': [key for key, items in groups.items()
                                                   if all(item.get('bytes') is None for item in items)]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out-dir', type=pathlib.Path, required=True)
    args = parser.parse_args()
    root = args.out_dir
    root.mkdir(parents=True, exist_ok=True)
    client = Metadata(root / 'metadata')
    products = catalog_products(client.fetch(CATALOG))
    # Start at all legacy grid directories, plus global map/model locations
    # linked by matching catalog titles. Keep non-NOAA/external targets visible.
    selected = [p for p in products if re.search(r'grid|anomal|isostat|geoid|deflection|geopotential', p['name'], re.I)
                and not re.search(r'station|profile|magnetic|TerrainBase', p['name'], re.I)]
    roots = {DATA + 'grids/', OCEAN_MAPS}
    roots.update(p['url'] for p in selected if p['url'].startswith(DATA) and p['url'].endswith('/'))
    print(f'Inventorying {len(roots)} catalog-linked roots; metadata only', flush=True)
    files, errors, directories = crawl(client, roots)
    print(f'Found {len(files)} files in {len(directories)} directory listings; reading sizes', flush=True)
    measure(client, files)
    print('Reading small header/readme files to identify map quantities', flush=True)
    descriptions = describe(client, files)
    inventory = {'retrieved_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
                 'catalog_url': CATALOG, 'products': products, 'selected_catalog_products': selected,
                 'roots': sorted(roots), 'directories': directories, 'directory_errors': errors,
                 'excluded_catalog_targets': [p for p in selected if not p['url'].startswith(DATA)],
                 'scope': 'Catalog-linked legacy grid directories, selected map/model directories, and the linked 2008 ocean-gravity/roughness products; excludes raw ship tracks, station-only collections, software and lists subdirectories. Header-based classification separates gravity maps from reference layers and model inputs.',
                 'summary': summarize(files), 'descriptions': descriptions,
                 'map_summary': map_summary(files, descriptions),
                 'files': sorted(files, key=lambda f: f['url'])}
    (root / 'inventory.json').write_text(json.dumps(inventory, indent=2) + '\n')
    print(json.dumps(inventory['summary'], indent=2), flush=True)
    print(json.dumps(inventory['map_summary']['categories'], indent=2), flush=True)
    print(f'Directory failures: {len(errors)}; external targets: {len(inventory["excluded_catalog_targets"])}', flush=True)


if __name__ == '__main__':
    main()
