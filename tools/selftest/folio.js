// Folio opened with sample.txt: the text is shown, an edit is saved back to the same file.
__t.run('folio', async () => {
  const tab = await __t.until('the file', () => S.tabs.find((t) => t.ta.value.includes('hello from the test')));
  tab.ta.value += 'edited by the test\n';
  tab.ta.dispatchEvent(new Event('input', { bubbles: true }));
  await saveTab(tab);
  await __t.until('the save', () => !isDirty(tab));
  return { tabs: S.tabs.length, path: tab.path };
});
