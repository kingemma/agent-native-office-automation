import { createApp } from 'vue'
import { createPinia } from 'pinia'
import Antd from 'ant-design-vue'
import 'ant-design-vue/dist/reset.css'
import './styles.css'
import { i18n } from './i18n'
import App from './App.vue'

createApp(App).use(createPinia()).use(i18n).use(Antd).mount('#app')
