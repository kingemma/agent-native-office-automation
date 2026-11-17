/**
 * 全局组件类型声明：main.ts 里 app.use(Antd) 已全局注册所有 ant-design-vue 组件，
 * 这里通过 GlobalComponents 让 vue-tsc 把模板中的 <a-xxx> 识别为组件，
 * 用宽松类型避免不同版本组件 props 声明差异造成的类型噪声。
 */
export {}

declare module 'vue' {
  export interface GlobalComponents {
    [name: string]: any
  }
}
