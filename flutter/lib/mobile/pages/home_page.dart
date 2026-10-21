import 'package:flutter/material.dart';
import 'package:flutter_hbb/mobile/pages/settings_page.dart';
import 'package:get/get.dart';
import '../../common.dart';
import '../../common/widgets/peer_tab_page.dart';
import '../../models/platform_model.dart';
import '../../models/state_model.dart';

abstract class PageShape extends Widget {
  final String title = "";
  final Widget icon = Icon(null);
  final List<Widget> appBarActions = [];
}

/// 设备页: 设备列表 + 最近会话, 点条目发起连接
class DevicesPage extends StatelessWidget implements PageShape {
  @override
  final icon = const Icon(Icons.devices);
  @override
  final title = translate("Devices");
  @override
  final appBarActions = const <Widget>[];

  @override
  Widget build(BuildContext context) =>
      PeerTabPage().marginOnly(top: 2, left: 10, right: 10);
}

class HomePage extends StatefulWidget {
  static final homeKey = GlobalKey<HomePageState>();

  HomePage() : super(key: homeKey);

  @override
  HomePageState createState() => HomePageState();
}

class HomePageState extends State<HomePage> {
  var _selectedIndex = 0;
  int get selectedIndex => _selectedIndex;
  final List<PageShape> _pages = [];

  void refreshPages() {
    setState(() {
      initPages();
    });
  }

  @override
  void initState() {
    super.initState();
    initPages();
  }

  void initPages() {
    _pages.clear();
    // 只保留设备列表 + 设置, 直连/被控/文件传输入口已移除
    if (!bind.isIncomingOnly()) {
      _pages.add(DevicesPage());
    }
    _pages.add(SettingsPage());
  }

  @override
  Widget build(BuildContext context) {
    return WillPopScope(
        onWillPop: () async {
          if (_selectedIndex != 0) {
            setState(() {
              _selectedIndex = 0;
            });
          } else {
            return true;
          }
          return false;
        },
        child: Scaffold(
          // backgroundColor: MyTheme.grayBg,
          appBar: AppBar(
            centerTitle: true,
            title: appTitle(),
            actions: _pages.elementAt(_selectedIndex).appBarActions,
          ),
          bottomNavigationBar: BottomNavigationBar(
            key: navigationBarKey,
            items: _pages
                .map((page) =>
                    BottomNavigationBarItem(icon: page.icon, label: page.title))
                .toList(),
            currentIndex: _selectedIndex,
            type: BottomNavigationBarType.fixed,
            selectedItemColor: MyTheme.accent, //
            unselectedItemColor: MyTheme.darkGray,
            onTap: (index) => setState(() {
              _selectedIndex = index;
            }),
          ),
          body: _pages.elementAt(_selectedIndex),
        ));
  }

  Widget appTitle() {
    return Text(bind.mainGetAppNameSync());
  }
}

class WebHomePage extends StatelessWidget {
  @override
  Widget build(BuildContext context) {
    stateGlobal.isInMainPage = true;
    return Scaffold(
      appBar: AppBar(
        centerTitle: true,
        title: Text(bind.mainGetAppNameSync()),
      ),
      body: PeerTabPage().marginOnly(top: 2, left: 10, right: 10),
    );
  }
}
