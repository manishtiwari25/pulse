import type { ComponentChildren } from 'preact';
import { PostLink, ThreadLink } from './ui/who';
import { DataTable } from './ui/table';

// A small CommonMark subset rendered to elements whose every leaf is a text
// node: paragraphs, headings, lists (nested, lazy continuation), fenced code,
// blockquotes, tables, rules, inline code, bold, italic, and links whose
// targets are board posts or threads. Any other target is shown as text.

const EXTERNAL = /^https?:\/\//;

function linkTarget(target: string, label: string): ComponentChildren {
  const t = String(target || '').trim();
  const lab = label || t;
  if (EXTERNAL.test(t)) {
    return (
      <span>
        {lab}
        {lab === t ? null : <> <code>{t}</code></>}
      </span>
    );
  }
  if (/^[0-9]{8}T[0-9]{6}Z-[A-Za-z0-9_.-]+$/.test(t)) return <PostLink id={t} label={lab} />;
  const postPath = t.match(/^docs\/board\/([^/]+)\/([^/]+)\/([^/]+?)(?:\.md)?$/);
  if (postPath) return <PostLink id={postPath[3]!} label={lab} />;
  const threadPath = t.match(/^docs\/board\/([^/]+)\/([^/]+)\/?$/);
  if (threadPath) return <ThreadLink k={threadPath[1] + '/' + threadPath[2]} />;
  return (
    <span>
      {lab}
      {lab === t ? null : <> <code>{t}</code></>}
    </span>
  );
}

export function inline(src: string): ComponentChildren[] {
  const out: ComponentChildren[] = [];
  const s = String(src || '');
  let i = 0;
  let k = 0;
  const add = (node: ComponentChildren) => out.push(typeof node === 'string' ? node : <span key={k++}>{node}</span>);
  while (i < s.length) {
    const starts = [s.indexOf('`', i), s.indexOf('[', i), s.indexOf('**', i), s.indexOf('*', i)].filter((n) => n >= 0).sort((a, b) => a - b);
    if (!starts.length) {
      add(s.slice(i));
      break;
    }
    const j = starts[0]!;
    if (j > i) add(s.slice(i, j));
    if (s.startsWith('`', j)) {
      const end = s.indexOf('`', j + 1);
      if (end < 0) {
        add(s.slice(j));
        break;
      }
      add(<code>{s.slice(j + 1, end)}</code>);
      i = end + 1;
      continue;
    }
    if (s.startsWith('[', j)) {
      const mid = s.indexOf('](', j + 1);
      const end = mid >= 0 ? s.indexOf(')', mid + 2) : -1;
      if (mid >= 0 && end >= 0) {
        add(linkTarget(s.slice(mid + 2, end), s.slice(j + 1, mid)));
        i = end + 1;
        continue;
      }
    }
    if (s.startsWith('**', j)) {
      const end = s.indexOf('**', j + 2);
      if (end >= 0) {
        add(<strong>{inline(s.slice(j + 2, end))}</strong>);
        i = end + 2;
        continue;
      }
    }
    if (s.startsWith('*', j)) {
      const end = s.indexOf('*', j + 1);
      if (end >= 0) {
        add(<em>{inline(s.slice(j + 1, end))}</em>);
        i = end + 1;
        continue;
      }
    }
    add(s[j]!);
    i = j + 1;
  }
  return out;
}

const LIST_RE = /^(\s*)([-*]|\d+\.)\s+(.*)$/;
const BLOCK_START = /^\s*(#{1,4}\s|```|>|(?:-{3,}|\*{3,})\s*$|\|)/;

function splitPipes(line: string): string[] {
  let x = line.trim();
  if (x.startsWith('|')) x = x.slice(1);
  if (x.endsWith('|')) x = x.slice(0, -1);
  return x.split('|').map((c) => c.trim());
}

export function Markdown({ source }: { source: string }) {
  const text = String(source || '');
  if (text.length > 200000) {
    return (
      <div className="md">
        <p className="text-muted">Large body shown as source to keep the page responsive.</p>
        <pre>{text}</pre>
      </div>
    );
  }
  const lines = text.replace(/\r\n?/g, '\n').split('\n');
  const blocks: ComponentChildren[] = [];
  let i = 0;
  let key = 0;
  const para: string[] = [];
  const flush = () => {
    if (para.length) {
      blocks.push(<p key={key++}>{inline(para.join(' '))}</p>);
      para.length = 0;
    }
  };
  const parseList = (baseIndent: number): ComponentChildren => {
    const first = lines[i]!.match(LIST_RE)!;
    const ordered = /\d+\./.test(first[2]!);
    const items: ComponentChildren[] = [];
    while (i < lines.length) {
      const m = lines[i]!.match(LIST_RE);
      if (!m || m[1]!.length !== baseIndent || /\d+\./.test(m[2]!) !== ordered) break;
      const parts = [m[3]!];
      let nested: ComponentChildren = null;
      i += 1;
      while (i < lines.length) {
        const cur = lines[i]!;
        if (!cur.trim()) {
          let j = i + 1;
          while (j < lines.length && !lines[j]!.trim()) j += 1;
          const nm = j < lines.length ? lines[j]!.match(LIST_RE) : null;
          if (nm && nm[1]!.length >= baseIndent) {
            i = j;
            if (nm[1]!.length === baseIndent) break;
            continue;
          }
          break;
        }
        const nm = cur.match(LIST_RE);
        if (nm) {
          if (nm[1]!.length > baseIndent) {
            nested = parseList(nm[1]!.length);
            continue;
          }
          break;
        }
        if (cur.search(/\S/) > baseIndent || !BLOCK_START.test(cur)) {
          parts.push(cur.trim());
          i += 1;
          continue;
        }
        break;
      }
      items.push(
        <li key={items.length}>
          {inline(parts.join(' '))}
          {nested}
        </li>,
      );
    }
    return ordered ? <ol key={key++}>{items}</ol> : <ul key={key++}>{items}</ul>;
  };
  while (i < lines.length) {
    const line = lines[i]!;
    if (!line.trim()) {
      flush();
      i += 1;
      continue;
    }
    const fence = line.match(/^```\s*([^`]*)\s*$/);
    if (fence) {
      flush();
      const body: string[] = [];
      i += 1;
      while (i < lines.length && !/^```\s*$/.test(lines[i]!)) {
        body.push(lines[i]!);
        i += 1;
      }
      if (i < lines.length) i += 1;
      const lang = fence[1]!.trim();
      blocks.push(
        <div key={key++} className="codeblock">
          {lang ? <div className="code-lang">{lang}</div> : null}
          <pre>{body.join('\n')}</pre>
        </div>,
      );
      continue;
    }
    if (/^\s*(-{3,}|\*{3,})\s*$/.test(line)) {
      flush();
      blocks.push(<hr key={key++} />);
      i += 1;
      continue;
    }
    const head = line.match(/^(#{1,4})\s+(.+)$/);
    if (head) {
      flush();
      const level = head[1]!.length + 1;
      const content = inline(head[2]!);
      blocks.push(level === 2 ? <h2 key={key++}>{content}</h2> : level === 3 ? <h3 key={key++}>{content}</h3> : level === 4 ? <h4 key={key++}>{content}</h4> : <h5 key={key++}>{content}</h5>);
      i += 1;
      continue;
    }
    if (/^>\s?/.test(line)) {
      flush();
      const qs: string[] = [];
      while (i < lines.length && /^>\s?/.test(lines[i]!)) {
        qs.push(lines[i]!.replace(/^>\s?/, ''));
        i += 1;
      }
      blocks.push(
        <blockquote key={key++}>
          <Markdown source={qs.join('\n')} />
        </blockquote>,
      );
      continue;
    }
    const list = line.match(LIST_RE);
    if (list && list[1]!.length <= 3) {
      flush();
      blocks.push(parseList(list[1]!.length));
      continue;
    }
    if (line.includes('|') && i + 1 < lines.length && /^\s*\|?\s*:?-{3,}:?\s*(\|\s*:?-{3,}:?\s*)+\|?\s*$/.test(lines[i + 1]!)) {
      flush();
      const headers = splitPipes(line);
      i += 2;
      const rows: string[][] = [];
      while (i < lines.length && lines[i]!.includes('|') && lines[i]!.trim()) {
        const cells = splitPipes(lines[i]!);
        while (cells.length < headers.length) cells.push('');
        rows.push(cells.slice(0, headers.length));
        i += 1;
      }
      blocks.push(<DataTable key={key++} columns={headers.map((h, ci) => ({ key: String(ci), label: h, render: (r: string[]) => inline(r[ci] || '') }))} rows={rows} rowKey={(_r, ri) => String(ri)} dense />);
      continue;
    }
    para.push(line.trim());
    i += 1;
  }
  flush();
  return <div className="md text-[13.5px] leading-relaxed">{blocks}</div>;
}
