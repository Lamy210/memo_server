import { marked } from 'marked';
import DOMPurify from 'dompurify';
import Prism from 'prismjs';
import 'prismjs/components/prism-typescript';
import 'prismjs/components/prism-rust';
import 'prismjs/components/prism-json';

// Markdownの設定
marked.setOptions({
  gfm: true,
  breaks: true,
  headerIds: true,
  mangle: false,
  sanitize: false
});

const renderer = new marked.Renderer();

// コードブロックのカスタマイズ
renderer.code = (code, language) => {
  language = language || 'plaintext';
  const highlighted = Prism.highlight(
    code,
    Prism.languages[language] || Prism.languages.plaintext,
    language
  );
  return `<pre><code class="language-${language}">${highlighted}</code></pre>`;
};

// チェックボックスのカスタマイズ
renderer.listitem = (text) => {
  if (/^\s*\[[x ]\]\s*/.test(text)) {
    text = text
      .replace(/^\s*\[ \]\s*/, '<input type="checkbox" disabled> ')
      .replace(/^\s*\[x\]\s*/, '<input type="checkbox" checked disabled> ');
    return `<li style="list-style: none">${text}</li>`;
  }
  return `<li>${text}</li>`;
};

export function convertMarkdown(markdown: string): string {
  const html = marked(markdown, { renderer });
  return DOMPurify.sanitize(html, {
    ALLOWED_TAGS: [
      'h1', 'h2', 'h3', 'h4', 'h5', 'h6',
      'blockquote', 'p', 'a', 'ul', 'ol', 
      'li', 'b', 'i', 'strong', 'em',
      'strike', 'code', 'hr', 'br', 'div',
      'table', 'thead', 'tbody', 'tr', 'th', 'td',
      'pre', 'input'
    ],
    ALLOWED_ATTR: ['href', 'target', 'rel', 'type', 'checked', 'disabled', 'class']
  });
}