export default {
  async handle({ value }: { value: { name: string } }) {
    return [{ kind: "Done", value: { greeting: `Hello, ${value.name}!` } }];
  },
};
