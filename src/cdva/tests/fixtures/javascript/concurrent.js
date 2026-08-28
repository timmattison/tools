export async function fetchName() {
  return "name";
}

test.concurrent("reads the name", async () => {
  expect(await fetchName()).toBe("name");
});
