# SPDX-License-Identifier: GPL-3.0-only
# JNA 在运行时查找 native 方法、结构字段和回调。
-dontwarn java.awt.**
-keep class com.sun.jna.** { *; }
-keep class io.github.groundgrounder.hortus.core.** extends com.sun.jna.Structure { *; }
-keep interface io.github.groundgrounder.hortus.core.** extends com.sun.jna.Library { *; }
-keep interface io.github.groundgrounder.hortus.core.** extends com.sun.jna.Callback { *; }
-dontwarn com.google.errorprone.annotations.**
-dontwarn javax.lang.model.**
