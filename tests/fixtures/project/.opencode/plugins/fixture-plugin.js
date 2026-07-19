export default async () => ({
  "shell.env": async (_input, output) => {
    output.env.FIXTURE_PLUGIN_LOADED = "fixture-plugin-loaded"
    output.env.NIX_CHILD_START_VERSION = process.env.NIX_FIXTURE_VERSION ?? "unset"
  },
})
