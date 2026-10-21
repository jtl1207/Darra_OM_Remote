import 'dart:math';

import 'package:bot_toast/bot_toast.dart';
import 'package:dynamic_layouts/dynamic_layouts.dart';
import 'package:flutter/material.dart';
import 'package:flutter_hbb/common/formatter/id_formatter.dart';
import 'package:flutter_hbb/common/widgets/peer_card.dart';
import 'package:flutter_hbb/common/widgets/peers_view.dart';
import 'package:flutter_hbb/desktop/widgets/popup_menu.dart';
import 'package:flutter_hbb/models/ab_model.dart';
import 'package:flutter_hbb/models/peer_model.dart';
import 'package:flutter_hbb/models/platform_model.dart';
import 'package:flutter_hbb/models/state_model.dart';
import '../../desktop/widgets/material_mod_popup_menu.dart' as mod_menu;
import 'package:get/get.dart';
import 'package:flex_color_picker/flex_color_picker.dart';

import '../../common.dart';
import 'dialog.dart';

final hideAbTagsPanel = false.obs;

class AddressBook extends StatefulWidget {
  final EdgeInsets? menuPadding;
  const AddressBook({Key? key, this.menuPadding}) : super(key: key);

  @override
  State<StatefulWidget> createState() {
    return _AddressBookState();
  }
}

class _AddressBookState extends State<AddressBook> {
  var menuPos = RelativeRect.fill;

  @override
  Widget build(BuildContext context) => Obx(() {
        return Column(
            children: [
              // NOT use Offstage to wrap LinearProgressIndicator
              if (gFFI.abModel.currentAbLoading.value &&
                  gFFI.abModel.currentAbEmpty)
                const LinearProgressIndicator(),
              buildErrorBanner(context,
                  loading: gFFI.abModel.currentAbLoading,
                  err: gFFI.abModel.abPullError,
                  retry: null,
                  close: gFFI.abModel.clearPullErrors),
              buildErrorBanner(context,
                  loading: gFFI.abModel.currentAbLoading,
                  err: gFFI.abModel.currentAbPushError,
                  retry: null, // remove retry
                  close: () => gFFI.abModel.currentAbPushError.value = ''),
              Expanded(
                child: Obx(() => stateGlobal.isPortrait.isTrue
                    ? _buildAddressBookPortrait()
                    : _buildAddressBookLandscape()),
              ),
            ],
        );
      });

  Widget _buildAddressBookLandscape() {
    return Row(
      children: [
        Offstage(
            offstage: hideAbTagsPanel.value,
            child: Container(
              decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(12),
                  border: Border.all(
                      color: Theme.of(context).colorScheme.background)),
              child: Container(
                width: 200,
                height: double.infinity,
                child: Column(
                  children: [
                    _buildTagHeader().marginOnly(
                        left: 8.0,
                        right: gFFI.abModel.legacyMode.value ? 8.0 : 0,
                        top: gFFI.abModel.legacyMode.value ? 8.0 : 0),
                    Expanded(
                      child: Container(
                        width: double.infinity,
                        height: double.infinity,
                        child: _buildTags(),
                      ),
                    ),
                  ],
                ),
              ),
            ).marginOnly(right: 12.0)),
        _buildPeersViews()
      ],
    );
  }

  Widget _buildAddressBookPortrait() {
    const padding = 8.0;
    return Column(
      children: [
        Offstage(
            offstage: hideAbTagsPanel.value,
            child: Container(
              decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(6),
                  border: Border.all(
                      color: Theme.of(context).colorScheme.background)),
              child: Container(
                padding:
                    const EdgeInsets.fromLTRB(padding, 0, padding, padding),
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    _buildTagHeader().marginOnly(left: 8.0, right: 0),
                    Container(
                      width: double.infinity,
                      child: _buildTags(),
                    ),
                  ],
                ),
              ),
            ).marginOnly(bottom: 12.0)),
        _buildPeersViews()
      ],
    );
  }

  Widget _buildTagHeader() {
    return Row(
      mainAxisAlignment: MainAxisAlignment.spaceBetween,
      children: [
        Text(translate('Tags')),
        Listener(
            onPointerDown: (e) {
              final x = e.position.dx;
              final y = e.position.dy;
              menuPos = RelativeRect.fromLTRB(x, y, x, y);
            },
            onPointerUp: (_) => _showMenu(menuPos),
            child: build_more(context, invert: true)),
      ],
    );
  }

  Widget _buildTags() {
    return Obx(() {
      List tags;
      if (gFFI.abModel.sortTags.value) {
        tags = gFFI.abModel.currentAbTags.toList();
        tags.sort();
      } else {
        tags = gFFI.abModel.currentAbTags.toList();
      }
      tags = [kUntagged, ...tags].toList();
      final editPermission = gFFI.abModel.current.canWrite();
      tagBuilder(String e) {
        return AddressBookTag(
            name: e,
            tags: gFFI.abModel.selectedTags,
            onTap: () {
              if (gFFI.abModel.selectedTags.contains(e)) {
                gFFI.abModel.selectedTags.remove(e);
              } else {
                gFFI.abModel.selectedTags.add(e);
              }
            },
            showActionMenu: editPermission);
      }

      gridView(bool isPortrait) => DynamicGridView.builder(
          shrinkWrap: isPortrait,
          gridDelegate: SliverGridDelegateWithWrapping(),
          itemCount: tags.length,
          itemBuilder: (BuildContext context, int index) {
            final e = tags[index];
            return tagBuilder(e);
          });
      final maxHeight = max(MediaQuery.of(context).size.height / 6, 100.0);
      return Obx(() => stateGlobal.isPortrait.isFalse
          ? gridView(false)
          : LimitedBox(maxHeight: maxHeight, child: gridView(true)));
    });
  }

  Widget _buildPeersViews() {
    return Expanded(
      child: Align(
          alignment: Alignment.topLeft,
          child: Obx(() => gFFI.abModel.groupByTag.value
              ? _buildGroupedPeers()
              : AddressBookPeersView(
                  menuPadding: widget.menuPadding,
                ))),
    );
  }

  /// 按 tag 分组渲染设备列表；每台设备只出现在首个命中分组，避免重复行。
  /// 受左侧选中 tag 筛选（与 AddressBookPeersView 同口径），组头可点快速筛选。
  Widget _buildGroupedPeers() {
    final selected = gFFI.abModel.selectedTags.toList();
    final order = <String>[];
    final groups = <String, List<Peer>>{};
    for (final p in gFFI.abModel.currentAbPeers) {
      final pt = p.tags.map((e) => e.toString()).toList();
      String? hit;
      if (selected.isEmpty) {
        hit = pt.isEmpty ? kUntagged : pt.first;
      } else {
        for (final t in pt) {
          if (selected.contains(t)) {
            hit = t;
            break;
          }
        }
        if (hit == null && pt.isEmpty && selected.contains(kUntagged)) {
          hit = kUntagged;
        }
      }
      if (hit == null) continue;
      if (!groups.containsKey(hit)) {
        groups[hit] = <Peer>[];
        order.add(hit);
      }
      groups[hit]!.add(p);
    }
    if (gFFI.abModel.sortTags.value) order.sort();
    return ListView(
      children: [
        for (final g in order) ...[
          _buildGroupHeader(g, groups[g]!.length),
          for (final p in groups[g]!)
            Container(
              height: 45,
              child: AddressBookPeerCard(
                  peer: p, menuPadding: widget.menuPadding),
            ).marginOnly(bottom: 4.0),
        ],
      ],
    );
  }

  /// tag 分组小节头：色点 + 名称 + 设备数，点按即只筛该 tag。
  Widget _buildGroupHeader(String tag, int count) {
    final color = gFFI.abModel.getCurrentAbTagColor(tag);
    return InkWell(
      onTap: () {
        final selected = gFFI.abModel.selectedTags;
        if (selected.length == 1 && selected.first == tag) {
          gFFI.abModel.unsetSelectedTags();
        } else {
          selected.value = [tag];
        }
      },
      child: Row(
        children: [
          Container(
            width: 10,
            height: 10,
            decoration: BoxDecoration(color: color, shape: BoxShape.circle),
          ).marginOnly(right: 6),
          Text(tag,
              style: TextStyle(fontWeight: FontWeight.bold, color: color)),
          const Spacer(),
          Text('$count', style: TextStyle(color: color)),
        ],
      ).marginOnly(top: 4.0, bottom: 4.0, right: 8.0),
    );
  }

  @protected
  MenuEntryBase<String> sortMenuItem() {
    final isOptFixed = isOptionFixed(sortAbTagsOption);
    return MenuEntrySwitch<String>(
      switchType: SwitchType.scheckbox,
      text: translate('Sort tags'),
      getter: () async {
        return shouldSortTags();
      },
      setter: (bool v) async {
        bind.mainSetLocalOption(
            key: sortAbTagsOption, value: v ? 'Y' : defaultOptionNo);
        gFFI.abModel.sortTags.value = v;
      },
      dismissOnClicked: true,
      enabled: (!isOptFixed).obs,
    );
  }

  @protected
  MenuEntryBase<String> filterMenuItem() {
    final isOptFixed = isOptionFixed(filterAbTagOption);
    return MenuEntrySwitch<String>(
      switchType: SwitchType.scheckbox,
      text: translate('Filter by intersection'),
      getter: () async {
        return filterAbTagByIntersection();
      },
      setter: (bool v) async {
        bind.mainSetLocalOption(
            key: filterAbTagOption, value: v ? 'Y' : defaultOptionNo);
        gFFI.abModel.filterByIntersection.value = v;
      },
      dismissOnClicked: true,
      enabled: (!isOptFixed).obs,
    );
  }

  @protected
  MenuEntryBase<String> groupMenuItem() {
    final isOptFixed = isOptionFixed(groupAbTagOption);
    return MenuEntrySwitch<String>(
      switchType: SwitchType.scheckbox,
      text: translate('Group by tag'),
      getter: () async {
        return shouldGroupAbTags();
      },
      setter: (bool v) async {
        bind.mainSetLocalOption(
            key: groupAbTagOption, value: v ? 'Y' : defaultOptionNo);
        gFFI.abModel.groupByTag.value = v;
      },
      dismissOnClicked: true,
      enabled: (!isOptFixed).obs,
    );
  }

  void _showMenu(RelativeRect pos) {
    final canWrite = gFFI.abModel.current.canWrite();
    final items = [
      if (canWrite) getEntry(translate("Add ID"), addIdToCurrentAb),
      if (canWrite) getEntry(translate("Add Tag"), abAddTag),
      getEntry(translate("Unselect all tags"), gFFI.abModel.unsetSelectedTags),
      if (gFFI.abModel.legacyMode.value)
        sortMenuItem(), // It's already sorted after pulling down
      filterMenuItem(),
      groupMenuItem(),
      if (gFFI.peerTabModel.multiSelectionMode)
        getEntry(translate("Tag selected devices"), abTagSelected),
    ];

    mod_menu.showMenu(
      context: context,
      position: pos,
      items: items
          .map((e) => e.build(
              context,
              MenuConfig(
                  commonColor: CustomPopupMenuTheme.commonColor,
                  height: CustomPopupMenuTheme.height,
                  dividerHeight: CustomPopupMenuTheme.dividerHeight)))
          .expand((i) => i)
          .toList(),
      elevation: 8,
    );
  }

  void addIdToCurrentAb() async {
    if (gFFI.abModel.isCurrentAbFull(true)) {
      return;
    }
    var isInProgress = false;
    var passwordVisible = false;
    IDTextEditingController idController = IDTextEditingController(text: '');
    TextEditingController aliasController = TextEditingController(text: '');
    TextEditingController passwordController = TextEditingController(text: '');
    TextEditingController noteController = TextEditingController(text: '');
    final tags = List.of(gFFI.abModel.currentAbTags);
    var selectedTag = List<dynamic>.empty(growable: true).obs;
    final style = TextStyle(fontSize: 14.0);
    String? errorMsg;
    final isCurrentAbShared = !gFFI.abModel.current.isPersonal();

    gFFI.dialogManager.show((setState, close, context) {
      submit() async {
        setState(() {
          isInProgress = true;
          errorMsg = null;
        });
        String id = idController.id;
        if (id.isEmpty) {
          // pass
        } else {
          if (gFFI.abModel.idContainByCurrent(id)) {
            setState(() {
              isInProgress = false;
              errorMsg = translate('ID already exists');
            });
            return;
          }
          var password = '';
          if (isCurrentAbShared) {
            password = passwordController.text;
          }
          String? errMsg2 = await gFFI.abModel.addIdToCurrent(
              id,
              aliasController.text.trim(),
              password,
              selectedTag,
              noteController.text);
          if (errMsg2 != null) {
            setState(() {
              isInProgress = false;
              errorMsg = errMsg2;
            });
            return;
          }
          // final currentPeers
        }
        close();
      }

      double marginBottom = 4;

      row({required Widget label, required Widget input}) {
        makeChild(bool isPortrait) => Row(
              children: [
                !isPortrait
                    ? ConstrainedBox(
                        constraints: const BoxConstraints(minWidth: 100),
                        child: label.marginOnly(right: 10))
                    : SizedBox.shrink(),
                Expanded(
                  child: ConstrainedBox(
                      constraints: const BoxConstraints(minWidth: 200),
                      child: input),
                ),
              ],
            ).marginOnly(bottom: !isPortrait ? 8 : 0);
        return Obx(() => makeChild(stateGlobal.isPortrait.isTrue));
      }

      return CustomAlertDialog(
        title: Text(translate("Add ID")),
        content: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Column(
              children: [
                row(
                    label: Row(
                      children: [
                        Text(
                          '*',
                          style: TextStyle(color: Colors.red, fontSize: 14),
                        ),
                        Text(
                          'ID',
                          style: style,
                        ),
                      ],
                    ),
                    input: Obx(() => TextField(
                          controller: idController,
                          inputFormatters: [IDTextInputFormatter()],
                          decoration: InputDecoration(
                              labelText: stateGlobal.isPortrait.isFalse
                                  ? null
                                  : translate('ID'),
                              errorText: errorMsg,
                              errorMaxLines: 5),
                        ).workaroundFreezeLinuxMint())),
                row(
                  label: Text(
                    translate('Alias'),
                    style: style,
                  ),
                  input: Obx(() => TextField(
                        controller: aliasController,
                        decoration: InputDecoration(
                          labelText: stateGlobal.isPortrait.isFalse
                              ? null
                              : translate('Alias'),
                        ),
                      ).workaroundFreezeLinuxMint()),
                ),
                if (isCurrentAbShared)
                  row(
                      label: Text(
                        translate('Password'),
                        style: style,
                      ),
                      input: Obx(
                        () => TextField(
                          controller: passwordController,
                          obscureText: !passwordVisible,
                          decoration: InputDecoration(
                            labelText: stateGlobal.isPortrait.isFalse
                                ? null
                                : translate('Password'),
                            suffixIcon: IconButton(
                              icon: Icon(
                                  passwordVisible
                                      ? Icons.visibility
                                      : Icons.visibility_off,
                                  color: MyTheme.lightTheme.primaryColor),
                              onPressed: () {
                                setState(() {
                                  passwordVisible = !passwordVisible;
                                });
                              },
                            ),
                          ),
                        ).workaroundFreezeLinuxMint(),
                      )),
                row(
                    label: Text(
                      translate('Note'),
                      style: style,
                    ),
                    input: Obx(
                      () => TextField(
                        controller: noteController,
                        maxLines: 3,
                        minLines: 1,
                        maxLength: 300,
                        decoration: InputDecoration(
                          labelText: stateGlobal.isPortrait.isFalse
                              ? null
                              : translate('Note'),
                        ),
                      ).workaroundFreezeLinuxMint(),
                    )),
                if (gFFI.abModel.currentAbTags.isNotEmpty)
                  Align(
                    alignment: Alignment.centerLeft,
                    child: Text(
                      translate('Tags'),
                      style: style,
                    ),
                  ).marginOnly(top: 8, bottom: marginBottom),
                if (gFFI.abModel.currentAbTags.isNotEmpty)
                  Align(
                    alignment: Alignment.centerLeft,
                    child: Wrap(
                      children: tags
                          .map((e) => AddressBookTag(
                              name: e,
                              tags: selectedTag,
                              onTap: () {
                                if (selectedTag.contains(e)) {
                                  selectedTag.remove(e);
                                } else {
                                  selectedTag.add(e);
                                }
                              },
                              showActionMenu: false))
                          .toList(growable: false),
                    ),
                  ),
              ],
            ),
            const SizedBox(
              height: 4.0,
            ),
            if (!gFFI.abModel.current.isPersonal())
              Row(children: [
                Icon(Icons.info, color: Colors.amber).marginOnly(right: 4),
                Text(
                  translate('share_warning_tip'),
                  style: TextStyle(fontSize: 12),
                )
              ]).marginSymmetric(vertical: 10),
            // NOT use Offstage to wrap LinearProgressIndicator
            if (isInProgress) const LinearProgressIndicator(),
          ],
        ),
        actions: [
          dialogButton("Cancel", onPressed: close, isOutline: true),
          dialogButton("OK", onPressed: submit),
        ],
        onSubmit: submit,
        onCancel: close,
      );
    });
  }

  /// 多选批量打标入口：新 tag 落到当前选中设备（云端 TagSync 同步）。
  void abTagSelected() async {
    if (gFFI.peerTabModel.selectedPeers.isEmpty) {
      BotToast.showText(
          contentColor: Colors.red, text: translate('No peer selected'));
      return;
    }
    abAddTag();
  }

  void abAddTag() async {
    var field = "";
    var msg = "";
    var isInProgress = false;
    TextEditingController controller = TextEditingController(text: field);
    gFFI.dialogManager.show((setState, close, context) {
      submit() async {
        setState(() {
          msg = "";
          isInProgress = true;
        });
        field = controller.text.trim();
        if (field.isEmpty) {
          // pass
        } else {
          final tags = field.trim().split(RegExp(r"[\s,;\n]+"));
          field = tags.join(',');
          for (var t in [kUntagged, translate(kUntagged)]) {
            if (tags.contains(t)) {
              BotToast.showText(
                  contentColor: Colors.red, text: 'Tag name cannot be "$t"');
              isInProgress = false;
              return;
            }
          }
          gFFI.abModel.addTags(tags);
          // final currentPeers
        }
        close();
      }

      return CustomAlertDialog(
        title: Text(translate("Add Tag")),
        content: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(translate("whitelist_sep")),
            const SizedBox(
              height: 8.0,
            ),
            Row(
              children: [
                Expanded(
                  child: TextField(
                    maxLines: null,
                    decoration: InputDecoration(
                      errorText: msg.isEmpty ? null : translate(msg),
                    ),
                    controller: controller,
                    autofocus: true,
                  ).workaroundFreezeLinuxMint(),
                ),
              ],
            ),
            const SizedBox(
              height: 4.0,
            ),
            // NOT use Offstage to wrap LinearProgressIndicator
            if (isInProgress) const LinearProgressIndicator(),
          ],
        ),
        actions: [
          dialogButton("Cancel", onPressed: close, isOutline: true),
          dialogButton("OK", onPressed: submit),
        ],
        onSubmit: submit,
        onCancel: close,
      );
    });
  }
}

class AddressBookTag extends StatelessWidget {
  final String name;
  final RxList<dynamic> tags;
  final Function()? onTap;
  final bool showActionMenu;

  const AddressBookTag(
      {Key? key,
      required this.name,
      required this.tags,
      this.onTap,
      this.showActionMenu = true})
      : super(key: key);

  @override
  Widget build(BuildContext context) {
    var pos = RelativeRect.fill;

    void setPosition(TapDownDetails e) {
      final x = e.globalPosition.dx;
      final y = e.globalPosition.dy;
      pos = RelativeRect.fromLTRB(x, y, x, y);
    }

    const double radius = 8;
    final isUnTagged = name == kUntagged;
    final showAction = showActionMenu && !isUnTagged;
    return GestureDetector(
      onTap: onTap,
      onTapDown: showAction ? setPosition : null,
      onSecondaryTapDown: showAction ? setPosition : null,
      onSecondaryTap: showAction ? () => _showMenu(context, pos) : null,
      onLongPress: showAction ? () => _showMenu(context, pos) : null,
      child: Obx(() => Container(
            decoration: BoxDecoration(
                color: tags.contains(name)
                    ? gFFI.abModel.getCurrentAbTagColor(name)
                    : Theme.of(context).colorScheme.background,
                borderRadius: BorderRadius.circular(4)),
            margin: const EdgeInsets.symmetric(horizontal: 4.0, vertical: 4.0),
            padding: const EdgeInsets.symmetric(vertical: 2.0, horizontal: 6.0),
            child: IntrinsicWidth(
              child: Row(
                children: [
                  if (!isUnTagged)
                    Container(
                      width: radius,
                      height: radius,
                      decoration: BoxDecoration(
                          shape: BoxShape.circle,
                          color: tags.contains(name)
                              ? Colors.white
                              : gFFI.abModel.getCurrentAbTagColor(name)),
                    ).marginOnly(right: radius / 2),
                  Expanded(
                    child: Text(isUnTagged ? translate(name) : name,
                        style: TextStyle(
                            overflow: TextOverflow.ellipsis,
                            color: tags.contains(name) ? Colors.white : null)),
                  ),
                ],
              ),
            ),
          )),
    );
  }

  void _showMenu(BuildContext context, RelativeRect pos) {
    final items = [
      getEntry(translate("Rename"), () {
        renameDialog(
            oldName: name,
            validator: (String? newName) {
              if (newName == null || newName.isEmpty) {
                return translate('Can not be empty');
              }
              if (newName != name &&
                  gFFI.abModel.currentAbTags.contains(newName)) {
                return translate('Already exists');
              }
              return null;
            },
            onSubmit: (String newName) {
              if (name != newName) {
                gFFI.abModel.renameTag(name, newName);
              }
              Future.delayed(Duration.zero, () => Get.back());
            },
            onCancel: () {
              Future.delayed(Duration.zero, () => Get.back());
            });
      }),
      getEntry(translate(translate('Change Color')), () async {
        final model = gFFI.abModel;
        Color oldColor = model.getCurrentAbTagColor(name);
        Color newColor = await showColorPickerDialog(
          context,
          oldColor,
          pickersEnabled: {
            ColorPickerType.accent: false,
            ColorPickerType.wheel: true,
          },
          pickerTypeLabels: {
            ColorPickerType.primary: translate("Primary Color"),
            ColorPickerType.wheel: translate("HSV Color"),
          },
          actionButtons: ColorPickerActionButtons(
              dialogOkButtonLabel: translate("OK"),
              dialogCancelButtonLabel: translate("Cancel")),
          showColorCode: true,
        );
        if (oldColor != newColor) {
          model.setTagColor(name, newColor);
        }
      }),
      getEntry(translate("Delete"), () {
        gFFI.abModel.deleteTag(name);
        Future.delayed(Duration.zero, () => Get.back());
      }),
    ];

    mod_menu.showMenu(
      context: context,
      position: pos,
      items: items
          .map((e) => e.build(
              context,
              MenuConfig(
                  commonColor: CustomPopupMenuTheme.commonColor,
                  height: CustomPopupMenuTheme.height,
                  dividerHeight: CustomPopupMenuTheme.dividerHeight)))
          .expand((i) => i)
          .toList(),
      elevation: 8,
    );
  }
}

MenuEntryButton<String> getEntry(String title, VoidCallback proc) {
  return MenuEntryButton<String>(
    childBuilder: (TextStyle? style) => Text(
      title,
      style: style,
    ),
    proc: proc,
    dismissOnClicked: true,
  );
}
