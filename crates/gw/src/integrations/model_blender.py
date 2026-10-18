# Blender (4.2+) batch script for `gw media add-model`. Run as
#   Blender -b --factory-startup -P model_blender.py -- '<json>'
# and prints one line `GWRESULT {json}`.
#
# mode "web": import a textured GLB, decimate every mesh to about `polycount` triangles (UVs and
#   materials kept), scale to the printed size in metres (so AR shows it life size), shrink
#   textures to `max_texture` px as JPEG, export a GLB (Draco-compressed when `draco`) and,
#   when `usdz` is set, a USDZ from the same decimated mesh for AR Quick Look.
# mode "stl": import an STL, decimate to `polycount`, export a binary STL, and report whether the
#   result is still closed (no non-manifold edges).
import bpy, bmesh, json, os, sys

a = json.loads(sys.argv[sys.argv.index("--") + 1])
bpy.ops.wm.read_factory_settings(use_empty=True)


def meshes():
    return [o for o in bpy.data.objects if o.type == 'MESH']


def tris():
    return sum(sum(len(p.vertices) - 2 for p in o.data.polygons) for o in meshes())


def decimate(target):
    before = tris()
    ratio = min(1.0, target / max(before, 1))
    if ratio < 1.0:
        for o in meshes():
            bpy.context.view_layer.objects.active = o
            m = o.modifiers.new("decimate", 'DECIMATE')
            m.decimate_type = 'COLLAPSE'
            m.ratio = ratio
            m.use_collapse_triangulate = True
            bpy.ops.object.modifier_apply(modifier=m.name)
    return before, tris()


def non_manifold_edges():
    n = 0
    for o in meshes():
        bm = bmesh.new()
        bm.from_mesh(o.data)
        n += sum(1 for e in bm.edges if not e.is_manifold)
        bm.free()
    return n


out = {}
if a["mode"] == "web":
    bpy.ops.import_scene.gltf(filepath=a["src"])
    before, after = decimate(a["polycount"])
    out.update(triangles_before=before, triangles=after)
    # Life size: the longest side becomes the printed size.
    size_m = a.get("size_m") or 0
    if size_m > 0:
        import mathutils
        lo = [1e30] * 3
        hi = [-1e30] * 3
        for o in meshes():
            for c in o.bound_box:
                w = o.matrix_world @ mathutils.Vector(c)
                for i in range(3):
                    lo[i] = min(lo[i], w[i])
                    hi[i] = max(hi[i], w[i])
        longest = max(hi[i] - lo[i] for i in range(3))
        if longest > 0:
            s = size_m / longest
            for o in bpy.data.objects:
                if o.parent is None:
                    o.matrix_world = mathutils.Matrix.Scale(s, 4) @ o.matrix_world
            out["scale"] = s
    # Every texture as a JPEG file at most max_texture px (packed scaled images would go out as PNG).
    tmp = a["tmp"]
    os.makedirs(tmp, exist_ok=True)
    cap = a["max_texture"]
    bpy.context.scene.render.image_settings.quality = a.get("jpeg_quality", 85)
    textures = []
    for n, im in enumerate(list(bpy.data.images)):
        w, h = im.size[:]
        if w == 0 or h == 0:
            continue
        if max(w, h) > cap:
            k = cap / max(w, h)
            im.scale(max(1, int(w * k)), max(1, int(h * k)))
        p = os.path.join(tmp, f"tex{n}.jpg")
        im.filepath_raw = p
        im.file_format = 'JPEG'
        im.save()
        if im.packed_file:
            im.unpack(method='REMOVE')
        im.filepath = p
        im.source = 'FILE'
        im.reload()
        textures.append([im.size[0], im.size[1]])
    out["textures"] = textures
    kw = dict(filepath=a["glb"], export_format='GLB', export_image_format='JPEG',
              export_jpeg_quality=a.get("jpeg_quality", 85), export_apply=True, export_yup=True)
    if a.get("draco"):
        kw.update(export_draco_mesh_compression_enable=True, export_draco_mesh_compression_level=6,
                  export_draco_position_quantization=14, export_draco_normal_quantization=10,
                  export_draco_texcoord_quantization=12)
    bpy.ops.export_scene.gltf(**kw)
    if a.get("usdz"):
        bpy.ops.wm.usd_export(filepath=a["usdz"], export_textures=True, export_materials=True,
                              generate_preview_surface=True, selected_objects_only=False,
                              export_uvmaps=True, export_normals=True)
elif a["mode"] == "stl":
    if hasattr(bpy.ops.wm, "stl_import"):
        bpy.ops.wm.stl_import(filepath=a["src"])
    else:
        bpy.ops.import_mesh.stl(filepath=a["src"])
    before_nm = non_manifold_edges()
    before, after = decimate(a["polycount"])
    out.update(triangles_before=before, triangles=after, non_manifold_before=before_nm,
               non_manifold_edges=non_manifold_edges())
    if hasattr(bpy.ops.wm, "stl_export"):
        bpy.ops.wm.stl_export(filepath=a["out"], ascii_format=False, apply_modifiers=True)
    else:
        bpy.ops.export_mesh.stl(filepath=a["out"], ascii=False, use_mesh_modifiers=True)
print("GWRESULT " + json.dumps(out), flush=True)
