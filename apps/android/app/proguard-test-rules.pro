# 仪器测试不需要裁剪或改写；测试 APK 体积不影响产品包。
-dontshrink
-dontoptimize
-dontobfuscate
-dontwarn javax.lang.model.**
-dontwarn com.google.errorprone.annotations.**
-keep class kotlin.** { *; }
-keep class kotlinx.** { *; }
