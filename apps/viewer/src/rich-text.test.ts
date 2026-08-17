import { describe, expect, it } from 'vitest';
import { htmlToText, plainToHtml, sanitizeReportHtml } from './rich-text';

describe('sanitizeReportHtml', () => {
  it('keeps report formatting tags', () => {
    const result = sanitizeReportHtml('<p><b>重点</b><br><i>描述</i></p><ul><li>一</li></ul>');
    expect(result).toContain('<b>重点</b>');
    expect(result).toContain('<i>描述</i>');
    expect(result).toContain('<ul><li>一</li></ul>');
  });

  it('removes script, style, event handlers and dangerous URLs', () => {
    const result = sanitizeReportHtml(
      '<script>alert(1)</script><p onclick="evil()">正文</p><img src=x onerror=evil()><a href="javascript:evil()">链接</a>',
    );
    expect(result).not.toContain('script');
    expect(result).not.toContain('onclick');
    expect(result).not.toContain('onerror');
    expect(result).not.toContain('javascript:');
    expect(result).not.toContain('<img');
    expect(result).not.toContain('<a');
    expect(result).toContain('正文');
  });
});

describe('htmlToText', () => {
  it('converts paragraph, br and list boundaries to readable lines', () => {
    const text = htmlToText('<p>第一行<br>第二行</p><ul><li>甲</li><li>乙</li></ul>');
    expect(text).toContain('第一行\n第二行');
    expect(text).toContain('甲');
    expect(text).toContain('乙');
  });

  it('treats formatting-only html as empty', () => {
    expect(htmlToText('<p><br></p>').trim()).toBe('');
    expect(htmlToText('&nbsp;').trim()).toBe('');
  });
});

describe('plainToHtml', () => {
  it('escapes markup and preserves line breaks', () => {
    expect(plainToHtml('<胸部> & 正常\n第二行')).toBe('&lt;胸部&gt; &amp; 正常<br>第二行');
  });
});
