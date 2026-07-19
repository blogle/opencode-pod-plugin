export default async () => ({
  "shell.env": async (_input, output) => {
    output.env.FIXTURE_PLUGIN_LOADED = "fixture-plugin-loaded"
  },
})
