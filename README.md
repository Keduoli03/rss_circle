# RSS Circle（友链朋友圈）

你是否经常烦恼于友链过多但没有时间浏览？那么 RSS Circle（友链朋友圈）将解决这一痛点。你可以随时获取友链网站的更新内容，并了解友链的活跃情况。

> 本项目是基于 [hexo-circle-of-friends](https://github.com/Rock-Candy-Tea/hexo-circle-of-friends)（原作者 [Rock-Candy-Tea](https://github.com/Rock-Candy-Tea)）的**二改版本**，由 [Keduoli03](https://github.com/Keduoli03) 在原项目基础上做了少量改动，并更名为 `rss_circle`。定位为通用的友链朋友圈服务，不再局限于 Hexo 博客。

即日起，项目恢复更新和维护，发布`v6.x.x`版本。

部署教程：[文档](https://fcircle-doc.yyyzyyyz.cn/)

### 抓取过滤与时间规则

在 `fc_settings.yaml` 中配置：

```yaml
# 整个博客停止抓取，填写域名即可；不同子域名需要分别填写。
BLOCK_BLOGS:
  - example.com
  - blog.example.org

# 仅过滤指定页面，使用文章完整 URL 的正则表达式。
BLOCK_ARTICLE:
  - '^https?://docs\.asoe\.cn/(friends|project)(/|$)'
```

- `BLOCK_BLOGS` 会跳过文章抓取和失联站点主页检测，不影响原友链源文件。下一次爬取会移除对应友链记录和已入库文章；不是只隐藏前端。
- `BLOCK_ARTICLE` 只过滤文章 URL，不屏蔽整站。下一次爬取也会清理已入库的匹配文章。两项均可设为 `[]`；老配置不填写时默认为空。
- 原有 `BLOCK_SITE` 正则与 `BLOCK_SITE_REVERSE` 保持可用；新域名黑名单始终生效，不受反向模式影响。
- 配置中的 `rss` / `suffix` 优先于自动探测；完全没有发布时间、更新时间的 feed 条目会被跳过，不再用抓取当天冒充发布时间。已入库文章保留最初记录的发布时间，更新时间仍正常刷新。
- `MAX_POSTS_NUM` 是**每轮每站最多采集篇数**，并非该博主历史文章总数上限。文章会保留到 `OUTDATE_CLEAN` 指定期限；本轮会先过滤、按发布时间排序、去重，再应用采集上限。

### 定时任务与 SQLite 维护

- GitHub Actions 默认每天北京时间 **08:00、20:00** 定时抓取（UTC cron 为 `0 0,12 * * *`）。GitHub 可能延迟调度；代码推送和手动触发仍然会执行，不计入这两次定时任务。
- 自托管 `CRON` 同步为 `0 8,20 * * *`，服务器需使用 `Asia/Shanghai` 时区。同一分支的 Actions 抓取任务串行执行，避免重叠写入数据。
- SQLite 第 3 号迁移会按完整文章 URL 去重，保留抓取时间较新的一份；同一时间则保留 ID 较大的一份。博客改名不会再生成第二份文章，后续写入使用 URL 唯一约束及原子 upsert。
- 增加发布时间、更新时间排序索引和友链 URL 查询索引。过期清理会同时删除已无对应文章的摘要；`OUTDATE_CLEAN: 0` 仍表示不删除过期文章，但会清理孤立摘要。
- 新迁移由 Rust core 在下一轮运行时执行。迁移前建议备份 `data.db`；不修改已有迁移文件，不将不同域名、不同路径的 URL 擅自合并。

### `v6.0.6` 更新说明：

- 🐛 **修复 `/all` 接口分页 bug**：修复 end=-1 时 `$limit` 传入负数导致 MongoDB 报错的问题
- 🐛 **修复 `/post` 接口换域名问题**：友链博主更换域名（且老域名重定向）后，老的 link 仍能查询到对应文章
- 🐛 **修复 MongoDB 集合名不一致**：统一 Rust core 与 Python API 的集合名为 `Post`/`Friend`（单数）
  - ⚠️ MongoDB 用户升级后，下一次定时任务运行后会自动重新填充数据；原 `Posts`/`Friends` 集合可手动删除
- 🐛 **修复 feed 日期解析**：当 `published`/`updated` 缺失时互相 fallback，避免文章日期显示为爬取当天
- 🚀 **性能优化**：mongodbapi 中 `last_updated_time` 查询改用 `find_one + sort` 替代 `limit(1000) + max()`

[![Stargazers over time](https://starchart.cc/Keduoli03/rss_circle.svg)](https://starchart.cc/Keduoli03/rss_circle)
