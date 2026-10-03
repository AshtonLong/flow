import { describe, expect, it } from 'vitest';
import { removeFillers } from '../../../src/main/cleanup/fillers';

describe('removeFillers', () => {
  describe('strips filler sounds', () => {
    it.each([
      ['at the start, recapitalising', 'Um, I think so', 'I think so'],
      ['at the start of a lower-case clause', 'um, i think so', 'I think so'],
      ['mid-sentence with both commas', 'I think, um, we should go', 'I think we should go'],
      ['mid-sentence without commas', 'so uh I was er going', 'so I was going'],
      ['before the full stop', 'I think um.', 'I think.'],
      ['before a question mark', 'What is it, um?', 'What is it?'],
      ['as its own sentence', 'Hello. Um. I think so', 'Hello. I think so'],
      ['at the start of a later sentence', 'Hello. Um, we think', 'Hello. We think'],
      ['in a run', 'um, uh, well', 'Well'],
      ['with a trailing ellipsis', 'It was, uh... fine', 'It was fine'],
      ['hmm', 'Hmm, let me see', 'Let me see'],
      ['erm and uhm', 'erm it is uhm fine', 'It is fine'],
      ['stretched spellings', 'ummm okay uhhh sure', 'Okay sure'],
      ['after a line break', 'Done.\num, next', 'Done.\nNext'],
      ['when nothing else was said', 'Um.', ''],
    ])('%s', (_name, input, expected) => {
      expect(removeFillers(input)).toBe(expected);
    });
  });

  describe('keeps look-alikes', () => {
    it.each([
      ['the ER was full'],
      ['Uh-huh sure'],
      ['umbrella under her error'],
      ['5 mm thick'],
      ['hummus and thumbs'],
      ['ah, I see'],
    ])('%s', (input) => {
      expect(removeFillers(input)).toBe(input);
    });
  });

  describe('collapses stuttered words', () => {
    it.each([
      ['a doubled pronoun', 'I I think', 'I think'],
      ['a doubled article', 'the the cat', 'the cat'],
      ['keeps the first capital', 'The the cat', 'The cat'],
      ['a triple', 'we we we should go', 'we should go'],
      ['with a comma between', 'I, I think', 'I think'],
      ['contractions', "I'm I'm going", "I'm going"],
      ['after a filler is removed', 'I um I think', 'I think'],
    ])('%s', (_name, input, expected) => {
      expect(removeFillers(input)).toBe(expected);
    });
  });

  describe('keeps legitimate doubles', () => {
    it.each([
      ['she had had enough'],
      ['I know that that is true'],
      ['what it is is a problem'],
      ['very very good'],
      ['no, no, not that'],
      ['I gave her her book'],
      ['call nine nine nine'],
      ['room 2 2 is free'],
      ["is isn't the same"],
      ['the theme'],
      ['I know. Know what?'],
    ])('%s', (input) => {
      expect(removeFillers(input)).toBe(input);
    });
  });

  it('handles the empty string', () => {
    expect(removeFillers('')).toBe('');
  });
});
