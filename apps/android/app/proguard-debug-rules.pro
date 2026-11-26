# 仪器测试通过类名和公开方法直接检查调试构建；保留应用自身 API，避免 R8 移除测试专用入口。
-keep class io.github.groundgrounder.hortus.** { *; }
-keep class kotlin.** { *; }
-keep class kotlinx.** { *; }
