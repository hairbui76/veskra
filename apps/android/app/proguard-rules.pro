# JNI entry points are looked up by name (Java_org_veskra_kit_NativeBridge_*): never rename/strip.
-keep class org.veskra.kit.NativeBridge { native <methods>; *; }
-keepclasseswithmembernames,includedescriptorclasses class * {
    native <methods>;
}
