# RSIAgent 状态角色

此目录提供透明 V3 3D 状态角色、可嵌入的 DOM 组件，以及只读取一个既有管理作业的本地服务。角色按现有 RSIAgent 事实显示文字和动作；动画没有业务写入能力。实际入口的页面、容器和画布背景均透明。

需要 Node.js 20 或更新版本及支持 WebGL2 的浏览器。没有 npm 依赖，不需要构建。

```sh
node bridge.mjs --preview --port 0
```

命令打印绑定在 127.0.0.1 的地址。预览默认显示“未连接”，不读取后端凭证或联系后端。浏览器核验页只在显式传入 `--qa` 时提供；该页面明确使用模拟事实。

```sh
node bridge.mjs --preview --qa --port 0
node --test tests/contracts.test.mjs
```

真实读取模式需要用户选定一个已存在的管理作业，并由可信启动环境提供与该作业所有者匹配的现有授权 token。服务固定后端地址与作业，浏览器不能切换 URL、身份或作业，也不会取得 token。启动真实模式的形式为：

```sh
node bridge.mjs --connect --backend http://127.0.0.1:7788 --job EXISTING_JOB_ID --port 0
```

当前代码从启动环境 `RSIA_HTTP_TOKEN` 取得既有 token。该凭证仍受后端 trusted-context 与 owner_actor/owner_role 合同限制；授权失败只会显示不可用。不要把 token 放入命令行、文件、页面或浏览器存储。本独立准备阶段没有执行真实连接。

嵌入时由宿主加载 `character.css`，并在其提供的专用空白区域挂载：

```js
import { createStatusCharacter } from './character.mjs';
const character = createStatusCharacter(container);
const token = character.selectSource('trusted-connection', existingJobId);
character.updateSnapshot(authorizedSnapshot, token, 'trusted-connection');
// 宿主卸载时：
character.dispose();
```

`selectSource` 撤销旧回调；同一 lease generation 的合法状态变化仍会更新。缺失、过期、撤销或无法解析的快照应由宿主调用 `showUnavailable`，立即停止工作动作。`status-source.mjs` 提供单请求读取链，隐藏和卸载时撤销请求；渲染器不读取网络。

默认尺寸为 320×340 CSS px，全身和特写镜头沿用 V3 参数。配色、回环、暂停、减少动态和旋转只影响展示。常驻画面允许点击穿透；主动“查看形象”才启用拖动、左右箭头与 Home 复位。可用区域不足 240px 时自动收起，使用原 V3 场景直接渲染的透明头部徽标，并释放全尺寸 GPU 资源；空间恢复后等待新状态。无法使用 WebGL2 时保留状态文字及 SVG 徽标，此时图形能力降级。

“已结束”只表示管理作业或票据的相应终态。它不表示验证通过、收益、进化、批准或采用。浏览器核验的模拟状态、截图和资源测试不提供任何后端业务验收信用。
