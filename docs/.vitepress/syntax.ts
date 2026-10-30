import type { LanguageRegistration, ThemeRegistration } from 'shiki'

import grammar from './stav.tmLanguage.json'

/** The TextMate grammar of Stav, registered with Shiki as `stav`. */
export const stavLanguage = { ...grammar, name: 'stav', aliases: ['stv'] } as unknown as LanguageRegistration

/** The colors a code theme gives each kind of token. */
interface Palette {
  name: string
  type: 'light' | 'dark'
  fg: string
  bg: string
  comment: string
  doc: string
  keyword: string
  operator: string
  punctuation: string
  type_: string
  string: string
  escape: string
  number: string
  func: string
  property: string
  block: string
  label: string
  self: string
  invalid: string
}

function theme(p: Palette): ThemeRegistration {
  return {
    name: p.name,
    type: p.type,
    colors: { 'editor.background': p.bg, 'editor.foreground': p.fg },
    fg: p.fg,
    bg: p.bg,
    tokenColors: [
      { scope: ['comment', 'punctuation.definition.comment'], settings: { foreground: p.comment, fontStyle: 'italic' } },
      { scope: ['comment.line.documentation', 'comment.block.documentation'], settings: { foreground: p.doc, fontStyle: 'italic' } },
      { scope: ['markup.inline.raw.stav'], settings: { foreground: p.doc, fontStyle: '' } },
      {
        scope: ['keyword', 'storage', 'storage.type', 'storage.modifier', 'keyword.control', 'keyword.other'],
        settings: { foreground: p.keyword },
      },
      { scope: ['keyword.other.priority'], settings: { foreground: p.keyword, fontStyle: 'italic' } },
      {
        scope: ['keyword.operator', 'punctuation.definition.generic', 'punctuation.separator.type'],
        settings: { foreground: p.operator },
      },
      {
        scope: ['punctuation', 'meta.brace', 'punctuation.separator', 'punctuation.accessor'],
        settings: { foreground: p.punctuation },
      },
      {
        scope: [
          'entity.name.type',
          'support.type',
          'entity.name.namespace',
          'entity.name.type.declaration',
          'support.class',
          'entity.name.class',
          'entity.other.inherited-class',
        ],
        settings: { foreground: p.type_ },
      },
      { scope: ['string', 'string.unquoted.path'], settings: { foreground: p.string } },
      { scope: ['string.unquoted.path'], settings: { foreground: p.string, fontStyle: 'italic' } },
      { scope: ['punctuation.definition.string'], settings: { foreground: p.string } },
      {
        scope: [
          'constant.character.escape',
          'punctuation.section.interpolation',
          'string.regexp',
        ],
        settings: { foreground: p.escape },
      },
      { scope: ['meta.interpolation', 'source.stav.embedded'], settings: { foreground: p.fg } },
      {
        scope: ['constant.numeric', 'constant.language', 'constant.other', 'support.constant'],
        settings: { foreground: p.number },
      },
      {
        scope: ['entity.name.function', 'support.function', 'meta.function-call', 'variable.function'],
        settings: { foreground: p.func },
      },
      {
        scope: [
          'variable.other.property',
          'variable.other.member',
          'support.type.property-name',
          'entity.name.tag.toml',
          'entity.name.tag.yaml',
          'meta.object-literal.key',
          'variable.other.object.property',
        ],
        settings: { foreground: p.property },
      },
      { scope: ['entity.name.tag', 'entity.name.section'], settings: { foreground: p.block, fontStyle: 'bold' } },
      { scope: ['entity.name.label', 'constant.other.symbol', 'entity.name.lifetime'], settings: { foreground: p.label } },
      { scope: ['constant.other.symbol'], settings: { foreground: p.label, fontStyle: 'italic' } },
      { scope: ['variable.language', 'variable.parameter'], settings: { foreground: p.self, fontStyle: 'italic' } },
      { scope: ['variable.other.declaration'], settings: { foreground: p.fg, fontStyle: 'bold' } },
      { scope: ['invalid', 'invalid.illegal'], settings: { foreground: p.invalid, fontStyle: 'underline' } },
      { scope: ['markup.heading', 'markup.bold'], settings: { foreground: p.keyword, fontStyle: 'bold' } },
      { scope: ['markup.italic'], settings: { fontStyle: 'italic' } },
      { scope: ['markup.inserted'], settings: { foreground: p.string } },
      { scope: ['markup.deleted'], settings: { foreground: p.invalid } },
    ],
  }
}

/** Frost: slate ink on a cold, pale page, with fjord teal and lichen. */
export const frost = theme({
  name: 'laft-frost',
  type: 'light',
  fg: '#1d2a31',
  bg: '#eaf0ef',
  comment: '#66767d',
  doc: '#4b6a3a',
  keyword: '#0a6a66',
  operator: '#4f6068',
  punctuation: '#66767d',
  type_: '#285ea0',
  string: '#5a6b06',
  escape: '#9a4d12',
  number: '#8a3f7c',
  func: '#14537a',
  property: '#9a3b2c',
  block: '#0a6a66',
  label: '#285ea0',
  self: '#9a3b2c',
  invalid: '#b3261e',
})

/** Night: the same roles on a deep slate, for dark pages. */
export const night = theme({
  name: 'laft-night',
  type: 'dark',
  fg: '#dce6e9',
  bg: '#101a1f',
  comment: '#7c8d94',
  doc: '#a4c48e',
  keyword: '#5fd1c8',
  operator: '#a6b6bc',
  punctuation: '#8b9ba1',
  type_: '#8fb8ec',
  string: '#c4dd6a',
  escape: '#f0b27a',
  number: '#e0a6d4',
  func: '#9fd0f0',
  property: '#f0a08c',
  block: '#5fd1c8',
  label: '#8fb8ec',
  self: '#f0a08c',
  invalid: '#ff8a7a',
})
