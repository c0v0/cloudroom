/** @jitsi/rnnoise-wasm 无类型声明，补充最小声明。 */
declare module "@jitsi/rnnoise-wasm" {
  interface RnnEmscriptenModule {
    locateFile?: (path: string) => string;
  }
  function createRNNWasmModule(options?: RnnEmscriptenModule): Promise<unknown>;
  export { createRNNWasmModule };
}

declare module "@jitsi/rnnoise-wasm/dist/rnnoise.wasm?url" {
  const url: string;
  export default url;
}
